//! Interface stub + concrete vtable global generation.
//!
//! For each polymorphic class found by the AST walker we emit:
//!
//! * one LLVM IR stub function per *originating* virtual method, and
//! * a `@class_vtable` global whose slots point at those stubs.
//!
//! Derived classes whose methods carry `OverrideAttr` reuse the
//! originating class's stub — every level of the hierarchy stores
//! exactly the same function pointer at each inherited vtable slot, so
//! SAW can symbolically merge through dispatch without losing
//! resolution.
//!
//! The complete `vtable_stubs.ll` is loadable via SAW's
//! `llvm_load_module` once assembled to `vtable_stubs.bc`
//! (see [`assemble_vtable_stubs`]).

use super::havoc::generate_havoc_spec;
use super::names::{sanitize_name};
use super::overrides::generate_override_index_with_vtable;
use super::types::{ir_default_return, method_param_ir_pieces, sret_inner_ir_type, type_to_llvm_ir};
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::GlobalVarInfo;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Emit vtable resolution stubs and havoc SAW specs for virtual methods.
///
/// Generates:
/// - `vtable_stubs.ll`: stubs + concrete vtable globals (LLVM text IR).
/// - `<class>_<method>_havoc_spec.saw`: one havoc spec per originating method.
/// - `interface_overrides.saw`: override bindings + per-class `alloc_<class>_this`.
pub fn emit_interface_stubs(
    methods: &[InterfaceMethod],
    constructors: &[ClassConstructor],
    globals: &[GlobalVarInfo],
    classes_with_vdtor: &HashSet<String>,
    output_dir: &Path,
    cryptol_fn: Option<&str>,
) -> Result<()> {
    if methods.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create {}", output_dir.display()))?;

    // Group methods by class. Each class's method list MUST be sorted by
    // source-location offset so vtable slots match MSVC's declaration-order
    // slot assignment.
    let mut by_class: BTreeMap<String, Vec<&InterfaceMethod>> = BTreeMap::new();
    for method in methods {
        by_class
            .entry(method.class_name.clone())
            .or_default()
            .push(method);
    }
    for class_methods in by_class.values_mut() {
        class_methods.sort_by_key(|m| m.source_offset);
    }

    // Write the LLVM IR stub file (no C compiler needed).
    let ll_path = output_dir.join("vtable_stubs.ll");
    let ll_content = generate_llvm_ir_stubs(&by_class, classes_with_vdtor);
    fs::write(&ll_path, &ll_content)
        .with_context(|| format!("Failed to write {}", ll_path.display()))?;

    // One havoc spec per *originating* method. Overrides reuse their
    // base's stub function (and therefore base's spec); generating a
    // separate spec for the derived class would reference a stub
    // symbol that doesn't exist.
    let layout_by_class: HashMap<&str, &ClassConstructor> = constructors
        .iter()
        .map(|c| (c.class_name.as_str(), c))
        .collect();
    for method in methods {
        if method.is_override {
            continue;
        }
        let filename = format!(
            "{}_{}_havoc_spec.saw",
            sanitize_name(&method.class_name),
            sanitize_name(&method.method.name),
        );
        let filepath = output_dir.join(&filename);
        let layout = layout_by_class.get(method.class_name.as_str()).copied();
        fs::write(&filepath, generate_havoc_spec(method, globals, layout, cryptol_fn))?;
    }

    let index_path = output_dir.join("interface_overrides.saw");
    let index_content =
        generate_override_index_with_vtable(methods, &by_class, constructors, classes_with_vdtor);
    fs::write(&index_path, index_content)?;
    Ok(())
}

/// Try to assemble `vtable_stubs.ll` to `vtable_stubs.bc` using either
/// `llvm-as` or `clang -c -emit-llvm`. SAW's `llvm_load_module` only
/// accepts bitcode, so doing this at generation time keeps the produced
/// `verify.saw` runnable without an external build step.
pub fn assemble_vtable_stubs(output_dir: &Path) -> AssembledStubs {
    let ll_path = output_dir.join("vtable_stubs.ll");
    if !ll_path.exists() {
        return AssembledStubs::NoStubs;
    }
    let bc_path = output_dir.join("vtable_stubs.bc");
    let candidates: &[(&str, &[&str])] = &[
        ("llvm-as", &[]),
        ("llvm-as.exe", &[]),
        ("clang", &["-c", "-emit-llvm"]),
        ("clang.exe", &["-c", "-emit-llvm"]),
        ("clang-cl", &["-c", "-emit-llvm"]),
        ("clang-cl.exe", &["-c", "-emit-llvm"]),
    ];
    for (cmd, extra_args) in candidates {
        match std::process::Command::new(cmd)
            .args(*extra_args)
            .arg(&ll_path)
            .arg("-o")
            .arg(&bc_path)
            .output()
        {
            Ok(out) if out.status.success() && bc_path.exists() => {
                return AssembledStubs::Bitcode {
                    bc_filename: "vtable_stubs.bc".to_string(),
                    assembler: (*cmd).to_string(),
                };
            }
            _ => continue,
        }
    }
    AssembledStubs::TextOnly {
        ll_filename: "vtable_stubs.ll".to_string(),
    }
}

/// Result of [`assemble_vtable_stubs`].
#[derive(Debug, Clone)]
pub enum AssembledStubs {
    /// `vtable_stubs.ll` was not present; nothing to assemble.
    NoStubs,
    /// Successfully assembled to bitcode.
    Bitcode { bc_filename: String, assembler: String },
    /// No assembler was available; the user must run `llvm-as` or
    /// `clang -c -emit-llvm` manually before running SAW.
    TextOnly { ll_filename: String },
}

impl AssembledStubs {
    /// Filename for `llvm_load_module` to reference. Returns `None` only
    /// when no stubs file exists at all.
    ///
    /// Always returns `vtable_stubs.bc` whenever stubs were generated —
    /// the emitted script prints a build-step warning when auto-assembly
    /// failed.
    pub fn script_filename(&self) -> Option<&str> {
        match self {
            AssembledStubs::NoStubs => None,
            AssembledStubs::Bitcode { bc_filename, .. } => Some(bc_filename.as_str()),
            AssembledStubs::TextOnly { .. } => Some("vtable_stubs.bc"),
        }
    }
}

/// Generate the LLVM IR text (`vtable_stubs.ll`) with stub functions and
/// concrete vtable globals for every class in `by_class`.
pub fn generate_llvm_ir_stubs(
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &HashSet<String>,
) -> String {
    let originating = compute_originating_classes(by_class);
    let resolve_stub_class = |m: &InterfaceMethod| -> String {
        if m.is_override {
            originating
                .get(&m.method.name)
                .cloned()
                .unwrap_or_else(|| m.class_name.clone())
        } else {
            m.class_name.clone()
        }
    };
    let stub_name_for = |m: &InterfaceMethod| -> String {
        let cls = resolve_stub_class(m);
        format!(
            "{}_{}_stub",
            sanitize_name(&cls).to_lowercase(),
            sanitize_name(&m.method.name).to_lowercase(),
        )
    };

    let mut out = String::new();
    out.push_str("; Auto-generated vtable stubs for SAW verification\n");
    out.push_str("; Load directly: m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("; Or assemble:   llvm-as vtable_stubs.ll -o vtable_stubs.bc\n");
    out.push_str(";\n");
    out.push_str("; SAW resolves indirect vtable calls through these:\n");
    out.push_str(";   this->vptr -> vtable[slot] -> stub function -> havoc spec\n\n");
    out.push_str("target datalayout = \"e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"\n");
    out.push_str("target triple = \"x86_64-pc-windows-msvc\"\n\n");

    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();
        let has_vdtor = classes_with_vdtor.contains(class_name.as_str());

        out.push_str(&format!("; ---- {class_name} vtable ----\n\n"));

        // MSVC ABI: a single deleting-destructor slot when the class has
        // a virtual dtor. Itanium would carry a pair (complete + deleting);
        // matching the wrong ABI shifts every method slot by one.
        if has_vdtor {
            out.push_str(&format!(
                "define void @{safe_class}_deleting_dtor_stub(ptr %self, i32 %flags) {{\n  ret void\n}}\n\n"
            ));
        }

        for method in class_methods {
            if method.is_override {
                continue;
            }
            emit_stub_for_method(&mut out, class_name, &stub_name_for(method), method);
        }

        let dtor_slots = if has_vdtor { 1 } else { 0 };
        let slot_count = class_methods.len() + dtor_slots;
        out.push_str(&format!("; Concrete vtable for {class_name}\n"));
        out.push_str(&format!(
            "@{safe_class}_vtable = global [{slot_count} x ptr] [\n"
        ));
        let mut first = true;
        if has_vdtor {
            out.push_str(&format!("  ptr @{safe_class}_deleting_dtor_stub"));
            first = false;
        }
        for method in class_methods {
            let stub_name = stub_name_for(method);
            if first {
                out.push_str(&format!("  ptr @{stub_name}"));
                first = false;
            } else {
                out.push_str(&format!(",\n  ptr @{stub_name}"));
            }
        }
        out.push_str("\n]\n\n");
    }
    out
}

/// Build `method_name → originating_class` for use when redirecting
/// override vtable slots.
fn compute_originating_classes(
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
) -> HashMap<String, String> {
    let mut originating: HashMap<String, String> = HashMap::new();
    for methods in by_class.values() {
        for m in methods {
            if !m.is_override {
                originating
                    .entry(m.method.name.clone())
                    .or_insert_with(|| m.class_name.clone());
            }
        }
    }
    originating
}

/// Emit one stub function definition. Handles MSVC ABI sret lowering
/// for aggregate returns (the sret pointer is inserted at index 1,
/// right after the implicit `this`).
fn emit_stub_for_method(
    out: &mut String,
    class_name: &str,
    stub_name: &str,
    method: &InterfaceMethod,
) {
    let param_strs = method_param_ir_pieces(&method.method);
    let (ret_ir, params_ir) = match sret_inner_ir_type(&method.method.return_type) {
        Some(inner) => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(this_p) = param_strs.first() {
                parts.push(this_p.clone());
            }
            parts.push(format!("ptr sret({inner}) %retptr"));
            for p in param_strs.iter().skip(1) {
                parts.push(p.clone());
            }
            ("void".to_string(), parts.join(", "))
        }
        None => (
            type_to_llvm_ir(&method.method.return_type),
            param_strs.join(", "),
        ),
    };
    out.push_str(&format!(
        "; {class_name}::{} [{}]\n",
        method.method.name,
        if method.is_pure { "pure virtual" } else { "virtual" },
    ));
    out.push_str(&format!(
        "define {ret_ir} @{stub_name}({params_ir}) {{\n"
    ));
    out.push_str(&format!("  {}\n", ir_default_return(&ret_ir)));
    out.push_str("}\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clang_ast::InterfaceMethod;
    use crate::constraints::{
        FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo,
    };
    use std::collections::HashSet;

    fn make_iface_method(class: &str, name: &str, ret: TypeInfo, offset: u64) -> InterfaceMethod {
        InterfaceMethod {
            class_name: class.into(),
            method: FunctionInfo {
                name: name.into(),
                mangled_name: None,
                params: vec![ParamInfo {
                    name: "this".into(),
                    ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                        name: "Self".into(),
                        size_bytes: 0,
                    })),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                }],
                return_type: ret,
                can_throw: false,
                is_virtual: true,
                has_body: false,
                is_system: false,
                annotations: vec![],
                referenced_globals: vec![],
                called_functions: vec![],
            },
            is_pure: true,
            is_override: false,
            source_offset: offset,
        }
    }

    #[test]
    fn test_vtable_slot_order_follows_source_offset() {
        let methods = vec![
            make_iface_method("IRV", "Authenticate", TypeInfo::Bool, 200),
            make_iface_method("IRV", "IsValidMetadataHeader", TypeInfo::Bool, 100),
        ];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_order");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let vtable_section = ll.split("@irv_vtable").nth(1).expect("vtable missing");
        let valid_pos = vtable_section
            .find("irv_isvalidmetadataheader_stub")
            .expect("IsValidMetadataHeader stub missing");
        let auth_pos = vtable_section
            .find("irv_authenticate_stub")
            .expect("Authenticate stub missing");
        assert!(valid_pos < auth_pos);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vtable_stub_uses_sret_for_large_struct_return() {
        let large = TypeInfo::Struct {
            name: "std::tuple<A,B>".into(),
            size_bytes: Some(48),
            fields: vec![],
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", large, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        assert!(stub_line.contains("define void"));
        assert!(stub_line.contains("sret([48 x i8])"));
        assert!(stub_line.contains("%retptr"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vtable_stub_no_sret_for_scalar_return() {
        let methods = vec![make_iface_method(
            "IFoo",
            "Tick",
            TypeInfo::SignedInt(32),
            100,
        )];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_no_sret");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ifoo_tick_stub"))
            .expect("Tick stub missing");
        assert!(stub_line.contains("define i32"));
        assert!(!stub_line.contains("sret"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vtable_stub_uses_sret_for_unsized_tuple_return() {
        let tuple_ret = TypeInfo::Opaque {
            name: "std::tuple<A,B>".into(),
            size_bytes: 0,
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", tuple_ret, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret_unsized");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        assert!(stub_line.contains("define void"));
        assert!(stub_line.contains("sret(["));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vtable_stub_sret_ordered_after_this() {
        let large = TypeInfo::Struct {
            name: "std::tuple<A,B>".into(),
            size_bytes: Some(48),
            fields: vec![],
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", large, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret_order");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        let this_pos = stub_line.find("%arg0").expect("this missing");
        let sret_pos = stub_line.find("%retptr").expect("sret missing");
        assert!(this_pos < sret_pos);
    }
}
