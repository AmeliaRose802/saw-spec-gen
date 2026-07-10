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
use super::vtable_ir::generate_llvm_ir_stubs;
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

    // Reclassify *orphan* overrides as originating methods. A method
    // marked `override` normally reuses its base class's stub + havoc
    // spec, so we skip emitting anything for it. But when the
    // originating base never appears among the parsed classes — an
    // external / unresolved base such as the parser's `"Unknown"`
    // fallback — there is no in-module base stub to reuse. Left as an
    // override it would (a) be dropped from havoc/binding emission,
    // yet (b) still occupy a vtable slot pointing at an undefined stub
    // symbol, so `vtable_stubs.ll` fails to assemble. Treating it as
    // originating gives it its own stub, havoc spec, and
    // `llvm_unsafe_assume_spec` binding — exactly how every other
    // external virtual call is modelled.
    let originating_names: HashSet<&str> = methods
        .iter()
        .filter(|m| !m.is_override)
        .map(|m| m.method.name.as_str())
        .collect();
    let owned_methods: Vec<InterfaceMethod> = methods
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if m.is_override && !originating_names.contains(m.method.name.as_str()) {
                m.is_override = false;
            }
            m
        })
        .collect();
    let methods = owned_methods.as_slice();

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
    fn test_orphan_override_gets_full_havoc_treatment() {
        // Regression for docs/07: a class whose virtual methods are all
        // `override`s of a base absent from the parsed set (the parser's
        // `"Unknown"` fallback). They must be modelled like any other
        // external virtual call — own stub + havoc spec + binding — not
        // left as dangling vtable slots or silent no-op stubs.
        let mut destroy = make_iface_method("Unknown", "Destroy", TypeInfo::Void, 100);
        destroy.is_override = true;
        let mut del = make_iface_method("Unknown", "DeleteThis", TypeInfo::Void, 200);
        del.is_override = true;
        let methods = vec![destroy, del];
        let dir = std::env::temp_dir().join("saw_spec_gen_orphan_override");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(&methods, &[], &[], &HashSet::new(), &dir, None, None).unwrap();

        // Both stubs are defined (vtable never dangles).
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        assert!(ll.contains("@unknown_destroy_stub("), "stub missing:\n{ll}");
        assert!(
            ll.contains("@unknown_deletethis_stub("),
            "stub missing:\n{ll}"
        );

        // A havoc spec file exists for each, and the override index
        // binds them (non-empty override list) — same as any external
        // virtual call, not a bare no-op.
        assert!(dir.join("Unknown_Destroy_havoc_spec.saw").exists());
        assert!(dir.join("Unknown_DeleteThis_havoc_spec.saw").exists());
        let idx = fs::read_to_string(dir.join("interface_overrides.saw")).unwrap();
        assert!(
            idx.contains("llvm_unsafe_assume_spec m \"unknown_destroy_stub\""),
            "expected havoc binding for orphan override:\n{idx}",
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
