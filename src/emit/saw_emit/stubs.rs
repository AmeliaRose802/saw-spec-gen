//! Interface stub + concrete vtable global generation.
//!
//! For each polymorphic class found by the AST walker we emit:
//!
//! * one LLVM IR stub function per *originating* virtual method, and
//! * a `@class_vtable` global whose slots point at those stubs
//!   (referenced by the per-class `alloc_<class>_this` helper in
//!   `interface_overrides.saw`), and
//! * on Itanium-ABI targets (Linux / macOS / *BSD), an *additional*
//!   `@_ZTV<mangled>` global with the Itanium leader-slot layout
//!   (`[null offset-to-top, null RTTI, stub1, stub2, ...]`) and
//!   `linkonce_odr` linkage. When the stub bitcode is linked into the
//!   compiled C++ bitcode via `llvm-link --override`, this global
//!   *replaces* the compiler-emitted vtable. Constructors that
//!   compute `this->vptr = _ZTV<C> + 16` therefore land on stub
//!   function pointers, and SAW's virtual dispatch resolves to the
//!   havoc spec instead of the real method body.
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
use super::names::sanitize_name;
use super::overrides::generate_override_index_with_vtable;
use super::types::{
    ir_default_return, method_param_ir_pieces, sret_inner_ir_type, type_to_llvm_ir,
};
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
///
/// `target_triple` is the `target triple = "..."` string from the
/// compiled C++ bitcode (None ⇒ assume MSVC). It selects the vtable
/// ABI layout — Itanium emits additional `_ZTV<mangled>` globals that
/// override the compiler's vtables; see the module-level docs.
pub fn emit_interface_stubs(
    methods: &[InterfaceMethod],
    constructors: &[ClassConstructor],
    globals: &[GlobalVarInfo],
    classes_with_vdtor: &HashSet<String>,
    output_dir: &Path,
    cryptol_fn: Option<&str>,
    target_triple: Option<&str>,
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
    let ll_content = generate_llvm_ir_stubs(&by_class, classes_with_vdtor, target_triple);
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
        fs::write(
            &filepath,
            generate_havoc_spec(method, globals, layout, cryptol_fn),
        )?;
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

/// Pre-link `main_bitcode` (the user's compiled code) with the
/// previously-assembled `vtable_stubs.bc` into a single `code.combined.bc`
/// using `llvm-link`. Lets the emitted `verify.saw` load one module via
/// `llvm_load_module` and skip `llvm_combine_modules` entirely — that
/// primitive was added to upstream SAW *after* the v1.5 release tag, so
/// generating scripts that depend on it forces users to build SAW from
/// source. Pre-linking with the off-the-shelf `llvm-link` tool sidesteps
/// the issue and produces output any SAW ≥ 0.x can load.
///
/// If `stubs` is anything other than `Bitcode` we just pass it through
/// unchanged — there's nothing to link with.
///
/// On link failure (e.g. `llvm-link` missing from PATH) the original
/// `Bitcode` value is returned, so the caller falls back to the
/// `llvm_combine_modules` emission path.
pub fn link_stubs_with_main(
    main_bitcode: &Path,
    output_dir: &Path,
    stubs: AssembledStubs,
) -> AssembledStubs {
    let stubs_bc_name = match &stubs {
        AssembledStubs::Bitcode { bc_filename, .. } => bc_filename.clone(),
        _ => return stubs,
    };
    let stubs_bc_path = output_dir.join(&stubs_bc_name);
    if !stubs_bc_path.exists() {
        return stubs;
    }
    if !main_bitcode.exists() {
        return stubs;
    }
    let combined_filename = "code.combined.bc";
    let combined_path = output_dir.join(combined_filename);
    let candidates: &[&str] = &["llvm-link", "llvm-link.exe"];
    for cmd in candidates {
        // `--override=<stubs.bc>` tells llvm-link to take the
        // definitions of any symbols present in the stub bitcode as
        // canonical, overriding the same-named symbols in the main
        // bitcode. This is what makes the Itanium-ABI `_ZTV<C>`
        // override actually win over the compiler-emitted vtable —
        // without it, llvm-link sees both as `linkonce_odr` and is
        // free to pick either. For MSVC-shaped stubs (which use
        // disjoint symbol names like `@<class>_vtable`) the flag is a
        // harmless no-op.
        let override_arg = format!("--override={}", stubs_bc_path.display());
        match std::process::Command::new(cmd)
            .arg(&override_arg)
            .arg(main_bitcode)
            .arg(&stubs_bc_path)
            .arg("-o")
            .arg(&combined_path)
            .output()
        {
            Ok(out) if out.status.success() && combined_path.exists() => {
                return AssembledStubs::LinkedBitcode {
                    combined_filename: combined_filename.to_string(),
                    linker: (*cmd).to_string(),
                };
            }
            Ok(_) | Err(_) => continue,
        }
    }
    stubs
}

/// Result of [`assemble_vtable_stubs`].
#[derive(Debug, Clone)]
pub enum AssembledStubs {
    /// `vtable_stubs.ll` was not present; nothing to assemble.
    NoStubs,
    /// Successfully assembled to bitcode.
    Bitcode {
        bc_filename: String,
        assembler: String,
    },
    /// `vtable_stubs.bc` was further pre-linked with the main bitcode
    /// into a single combined module. The emitted verify script can do
    /// one `llvm_load_module` and skip `llvm_combine_modules` (which
    /// the v1.5 SAW release tarball doesn't ship).
    LinkedBitcode {
        combined_filename: String,
        linker: String,
    },
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
            AssembledStubs::LinkedBitcode {
                combined_filename, ..
            } => Some(combined_filename.as_str()),
            AssembledStubs::TextOnly { .. } => Some("vtable_stubs.bc"),
        }
    }
}

/// Generate the LLVM IR text (`vtable_stubs.ll`) with stub functions and
/// concrete vtable globals for every class in `by_class`.
///
/// Always emits:
///   * one stub function per originating virtual method,
///   * an `@<class>_vtable = global [N x ptr] [...]` (used by the
///     per-class `alloc_<class>_this` SAW helper).
///
/// On Itanium-ABI targets, additionally emits:
///   * `@_ZTV<mangled> = linkonce_odr unnamed_addr constant
///     { [<N+2> x ptr] } { [<N+2> x ptr] [ptr null, ptr null, stub1,
///     stub2, ...] }, align 8` — a layout-matching override for the
///     compiler-emitted vtable that virtual constructors load from
///     (`store ptr getelementptr inbounds (..., @_ZTV<C>, ..., i32 2),
///     ptr %this`). When the stub bitcode is linked into the C++
///     bitcode via `llvm-link --override`, our stub `_ZTV<C>` wins and
///     virtual dispatch resolves to the stub function (which has a
///     havoc spec bound) instead of the real method.
pub fn generate_llvm_ir_stubs(
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &HashSet<String>,
    target_triple: Option<&str>,
) -> String {
    let abi = TargetAbi::from_triple(target_triple);
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
    out.push_str(abi.datalayout());
    out.push('\n');
    out.push_str(&format!("target triple = \"{}\"\n\n", abi.triple()));

    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();
        let has_vdtor = classes_with_vdtor.contains(class_name.as_str());

        out.push_str(&format!("; ---- {class_name} vtable ----\n\n"));

        // MSVC ABI: a single deleting-destructor slot when the class has
        // a virtual dtor. Itanium would carry a pair (complete + deleting);
        // matching the wrong ABI shifts every method slot by one.
        if has_vdtor {
            match abi {
                TargetAbi::Msvc => {
                    out.push_str(&format!(
                        "define void @{safe_class}_deleting_dtor_stub(ptr %self, i32 %flags) {{\n  ret void\n}}\n\n"
                    ));
                }
                TargetAbi::Itanium => {
                    // Itanium has two dtor slots: D1 (complete object)
                    // at slot 0, D0 (deleting) at slot 1.
                    out.push_str(&format!(
                        "define void @{safe_class}_complete_dtor_stub(ptr %self) {{\n  ret void\n}}\n\n"
                    ));
                    out.push_str(&format!(
                        "define void @{safe_class}_deleting_dtor_stub(ptr %self) {{\n  ret void\n}}\n\n"
                    ));
                }
            }
        }

        for method in class_methods {
            if method.is_override {
                continue;
            }
            emit_stub_for_method(&mut out, class_name, &stub_name_for(method), method);
        }

        // MSVC-style flat vtable. Kept on every target — the
        // `alloc_<class>_this` SAW helper references this global via
        // `llvm_global_initializer "<class>_vtable"` and is portable
        // across ABIs.
        let msvc_dtor_slots = if has_vdtor { 1 } else { 0 };
        let msvc_slot_count = class_methods.len() + msvc_dtor_slots;
        out.push_str(&format!("; Concrete vtable for {class_name} (MSVC-style, used by alloc_{safe_class}_this)\n"));
        out.push_str(&format!(
            "@{safe_class}_vtable = global [{msvc_slot_count} x ptr] [\n"
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

        // Itanium ABI: also emit a `_ZTV<MangledClass>` override with
        // leader slots `[offset-to-top, RTTI]` followed by (D1, D0)
        // dtor slots when applicable, then methods in declaration
        // order. Linking with `llvm-link --override=vtable_stubs.bc`
        // makes this definition win over the compiler-emitted one so
        // virtual dispatch from `new C()` resolves to stubs.
        if matches!(abi, TargetAbi::Itanium) {
            let itanium_dtor_slots = if has_vdtor { 2 } else { 0 };
            let itanium_slot_count = 2 + itanium_dtor_slots + class_methods.len();
            let mangled = itanium_mangle_class_name(class_name);
            let symbol = format!("_ZTV{mangled}");
            out.push_str(&format!(
                "; Itanium-ABI vtable override for {class_name} ({symbol}).\n"
            ));
            out.push_str(&format!(
                "; Replaces the compiler-emitted vtable when linked via `llvm-link --override`.\n"
            ));
            out.push_str(&format!(
                "@{symbol} = linkonce_odr unnamed_addr constant {{ [{itanium_slot_count} x ptr] }} {{ [{itanium_slot_count} x ptr] [\n"
            ));
            // Leader slots: offset-to-top and RTTI pointer. We emit
            // `null` for both — RTTI is only consulted by `dynamic_cast`
            // / `typeid`, neither of which appear in havoc proofs, and
            // offset-to-top is unused outside of multi-inheritance
            // adjustor thunks.
            out.push_str("  ptr null,\n");
            out.push_str("  ptr null");
            if has_vdtor {
                out.push_str(&format!(
                    ",\n  ptr @{safe_class}_complete_dtor_stub,\n  ptr @{safe_class}_deleting_dtor_stub"
                ));
            }
            for method in class_methods {
                let stub_name = stub_name_for(method);
                out.push_str(&format!(",\n  ptr @{stub_name}"));
            }
            out.push_str("\n] }, align 8\n\n");
        }
    }
    out
}

/// LLVM target ABI flavour.  Selected from the input bitcode's
/// `target triple` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetAbi {
    /// Windows-MSVC C++ ABI. Flat vtable, single deleting-dtor slot,
    /// MSVC-style mangled vtable symbols (`??_7C@@6B@`).
    Msvc,
    /// Itanium C++ ABI (Linux / macOS / *BSD / MinGW). Vtable has two
    /// leader slots (offset-to-top, RTTI), two dtor slots (D1, D0),
    /// `_ZTV<mangled>` mangled symbol.
    Itanium,
}

impl TargetAbi {
    fn from_triple(triple: Option<&str>) -> Self {
        match triple {
            // Explicit MSVC stays MSVC. Cygwin / MinGW (`windows-gnu`)
            // actually uses Itanium for vtable layout, so don't lump it
            // in with MSVC here.
            Some(t) if t.contains("windows-msvc") => TargetAbi::Msvc,
            // Anything else with a triple → Itanium. Most common cases:
            // `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
            // `x86_64-pc-windows-gnu` (MinGW).
            Some(_) => TargetAbi::Itanium,
            // No triple given → preserve historical default. The
            // original `vtable_stubs.ll` hard-coded the MSVC triple and
            // every existing test exercises the MSVC layout.
            None => TargetAbi::Msvc,
        }
    }

    fn triple(self) -> &'static str {
        match self {
            TargetAbi::Msvc => "x86_64-pc-windows-msvc",
            TargetAbi::Itanium => "x86_64-unknown-linux-gnu",
        }
    }

    fn datalayout(self) -> &'static str {
        match self {
            // MSVC datalayout — matches `clang -target x86_64-pc-windows-msvc`.
            TargetAbi::Msvc =>
                "target datalayout = \"e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"",
            // Itanium / SysV datalayout — matches `clang -target x86_64-unknown-linux-gnu`.
            // The `m:` mangling token (`m:e` vs `m:w`) is what differs
            // most visibly from MSVC and is the only piece llvm-link
            // cares about when merging modules.
            TargetAbi::Itanium =>
                "target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"",
        }
    }
}

/// Mangle a C++ class name into the Itanium-ABI form that follows the
/// `_ZTV` prefix.  Handles top-level (`BadProcessor` → `12BadProcessor`)
/// and `::`-namespaced (`foo::Bar` → `N3foo3BarE`) cases.  Stripped of
/// any leading `class `/`struct ` keyword that clang may attach.
///
/// Does **not** handle templates (`Foo<int>`), anonymous namespaces, or
/// non-ASCII identifiers — none of which appear in the C++ verification
/// demos. If a future demo needs them, switch to a real Itanium mangler
/// (e.g. via `cpp_demangle`'s inverse) instead of extending this.
fn itanium_mangle_class_name(class_name: &str) -> String {
    let name = class_name
        .trim_start_matches("class ")
        .trim_start_matches("struct ")
        .trim();
    if name.contains("::") {
        let mut out = String::from("N");
        for part in name.split("::") {
            // ASCII byte length matches what Itanium expects for plain
            // identifiers; reject (silently fall back to byte-count)
            // for anything weirder.
            out.push_str(&part.len().to_string());
            out.push_str(part);
        }
        out.push('E');
        out
    } else {
        format!("{}{}", name.len(), name)
    }
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
        if method.is_pure {
            "pure virtual"
        } else {
            "virtual"
        },
    ));
    out.push_str(&format!("define {ret_ir} @{stub_name}({params_ir}) {{\n"));
    out.push_str(&format!("  {}\n", ir_default_return(&ret_ir)));
    out.push_str("}\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clang_ast::InterfaceMethod;
    use crate::constraints::{FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo};
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
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
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
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
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
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
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
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
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
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        let this_pos = stub_line.find("%arg0").expect("this missing");
        let sret_pos = stub_line.find("%retptr").expect("sret missing");
        assert!(this_pos < sret_pos);
    }

    #[test]
    fn test_itanium_emits_ztv_override_with_leader_slots() {
        // Itanium triple → emit `_ZTV<mangled>` with `[null, null,
        // stub1, stub2, ...]` layout in addition to the MSVC-style
        // `@<class>_vtable`.
        let methods = vec![
            make_iface_method("BadProcessor", "validate", TypeInfo::Void, 100),
            make_iface_method("BadProcessor", "audit", TypeInfo::Void, 200),
        ];
        let dir = std::env::temp_dir().join("saw_spec_gen_itanium_ztv");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &HashSet::new(),
            &dir,
            None,
            Some("x86_64-unknown-linux-gnu"),
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        assert!(
            ll.contains("target triple = \"x86_64-unknown-linux-gnu\""),
            "expected Itanium triple, got:\n{ll}",
        );
        assert!(
            ll.contains("@badprocessor_vtable"),
            "MSVC-style helper vtable should still be emitted (used by alloc_<class>_this)",
        );
        // 2 leader slots + 2 methods = 4 slots wrapped in a struct.
        assert!(
            ll.contains("@_ZTV12BadProcessor = linkonce_odr unnamed_addr constant { [4 x ptr] }"),
            "expected Itanium `_ZTV12BadProcessor` override with 4 slots, got:\n{ll}",
        );
        // Leader-slot signature: two `ptr null` entries before any stub.
        let ztv_section = ll
            .split("@_ZTV12BadProcessor")
            .nth(1)
            .expect("ZTV body missing");
        let leader_block = &ztv_section[..ztv_section.find("], align").unwrap_or(ztv_section.len())];
        assert!(
            leader_block.matches("ptr null").count() >= 2,
            "expected two `ptr null` leader slots, got:\n{leader_block}",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_msvc_default_omits_ztv_override() {
        // No triple → MSVC default → no `_ZTV` symbol should appear.
        let methods = vec![
            make_iface_method("BadProcessor", "validate", TypeInfo::Void, 100),
        ];
        let dir = std::env::temp_dir().join("saw_spec_gen_msvc_no_ztv");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        assert!(ll.contains("target triple = \"x86_64-pc-windows-msvc\""));
        assert!(!ll.contains("_ZTV"), "MSVC mode should not emit Itanium `_ZTV` symbols:\n{ll}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_itanium_mangle_class_name_simple_and_namespaced() {
        assert_eq!(itanium_mangle_class_name("BadProcessor"), "12BadProcessor");
        assert_eq!(itanium_mangle_class_name("foo::Bar"), "N3foo3BarE");
        // Tolerates `class `/`struct ` keyword prefix from clang.
        assert_eq!(itanium_mangle_class_name("class BadProcessor"), "12BadProcessor");
    }

    #[test]
    fn test_target_abi_from_triple_picks_correct_flavour() {
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-pc-windows-msvc")),
            TargetAbi::Msvc,
        );
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-unknown-linux-gnu")),
            TargetAbi::Itanium,
        );
        assert_eq!(
            TargetAbi::from_triple(Some("aarch64-apple-darwin")),
            TargetAbi::Itanium,
        );
        // MinGW (windows-gnu) uses the Itanium ABI for vtable layout
        // even though the OS is Windows.
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-pc-windows-gnu")),
            TargetAbi::Itanium,
        );
        assert_eq!(TargetAbi::from_triple(None), TargetAbi::Msvc);
    }
}
