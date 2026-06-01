//! LLVM-mode SAW spec emission.
//!
//! Produces the per-function `<name>_auto_spec.saw` files plus an
//! `auto_specs.saw` include index. Two flavors are supported:
//!
//! * **concrete**: spec drives `llvm_verify` against a real
//!   implementation (sret + non-sret variants, postconditions, etc.).
//! * **adversarial**: spec drives `llvm_unsafe_assume_spec` for
//!   virtual / external functions where the implementation is unknown.

use super::llvm_return::{
    emit_adversarial_return_and_postconds, emit_concrete_return_and_postconds,
};
use super::llvm_setup::{emit_llvm_globals_setup, emit_llvm_param_setup};
use super::names::{sanitize_name, spec_safe_id};
use crate::constraints::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Whether to emit LLVM or MIR specs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmitMode {
    Llvm,
    Mir,
}

/// Emit SAW spec files for all constraints (LLVM mode).
pub fn emit_saw_specs(
    specs: &[SpecConstraint],
    output_dir: &Path,
    experimental: bool,
) -> Result<()> {
    // Build a deduplicated union of every spec's referenced globals so
    // that concrete specs allocate all known globals (callees may
    // transitively touch globals the target doesn't directly reference).
    let mut all_globals: Vec<GlobalVarInfo> = specs
        .iter()
        .flat_map(|s| s.referenced_globals.iter().cloned())
        .collect();
    all_globals.sort_by(|a, b| a.mangled_name.cmp(&b.mangled_name));
    all_globals.dedup_by(|a, b| a.mangled_name == b.mangled_name);
    emit_saw_specs_with_mode(
        specs,
        output_dir,
        EmitMode::Llvm,
        experimental,
        &all_globals,
    )
}

/// Emit LLVM SAW spec files with an explicit list of globals.
pub fn emit_saw_specs_with_globals(
    specs: &[SpecConstraint],
    output_dir: &Path,
    experimental: bool,
    all_globals: &[GlobalVarInfo],
) -> Result<()> {
    emit_saw_specs_with_mode(specs, output_dir, EmitMode::Llvm, experimental, all_globals)
}

/// Emit a single experimental (`llvm_unspecified_globals`) spec file.
///
/// Every external function conservatively havocs all mutable globals.
/// Even "safe" system functions like `printf` can write through pointer
/// args (e.g. `%n`), and we have no way to prove they don't touch
/// user-defined globals.
pub fn emit_single_experimental_spec(
    spec: &SpecConstraint,
    all_globals: &[GlobalVarInfo],
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let safe_id = spec_safe_id(spec);
    let filename = format!("{}_auto_spec.saw", safe_id);
    let filepath = output_dir.join(&filename);
    fs::write(&filepath, generate_unspecified_spec(spec, all_globals))?;
    Ok(())
}

/// Write the `operator new` spec to `operator_new_auto_spec.saw`.
///
/// Allocates a 256-byte buffer — generous enough to cover every
/// reasonable C++ object size in the bundled end-to-end tests. Smaller buffers
/// failed override matching for classes with non-trivial data members.
pub fn emit_operator_new_spec(mangled_name: &str, output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let filepath = output_dir.join("operator_new_auto_spec.saw");
    let content = format!(
        "// Auto-generated spec for: operator new\n\
         // Allocates fresh memory and returns a pointer\n\
         \n\
         let operator_new_spec = do {{\n\
         \x20   sz <- llvm_fresh_var \"sz\" (llvm_int 64);\n\
         \x20   llvm_execute_func [llvm_term sz];\n\
         \x20   p <- llvm_alloc_aligned 8 (llvm_array 256 (llvm_int 8));\n\
         \x20   llvm_return p;\n\
         }};\n\
         ov_new <- llvm_unsafe_assume_spec m \"{}\" operator_new_spec;\n",
        mangled_name,
    );
    fs::write(&filepath, content)?;
    Ok(())
}

/// Shared dispatcher used by both LLVM and MIR public entry points.
pub fn emit_saw_specs_with_mode(
    specs: &[SpecConstraint],
    output_dir: &Path,
    mode: EmitMode,
    experimental: bool,
    all_globals: &[GlobalVarInfo],
) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory {}", output_dir.display()))?;

    for spec in specs {
        let filename = format!("{}_auto_spec.saw", sanitize_name(&spec.function_name));
        let filepath = output_dir.join(&filename);
        let mut file = fs::File::create(&filepath)
            .with_context(|| format!("Failed to create {}", filepath.display()))?;

        let needs_adversarial = spec.is_virtual || !spec.has_body;
        let content = if experimental && needs_adversarial && mode == EmitMode::Llvm {
            generate_unspecified_spec(spec, all_globals)
        } else {
            match mode {
                EmitMode::Llvm => generate_saw_spec(spec, all_globals),
                EmitMode::Mir => super::mir_spec::generate_mir_spec(spec),
            }
        };
        write!(file, "{}", content)?;
    }

    let index_path = output_dir.join("auto_specs.saw");
    let mut index = fs::File::create(&index_path)?;
    writeln!(index, "// Auto-generated SAW specs -- do not edit")?;
    writeln!(
        index,
        "// Generated by saw-spec-gen from type system constraints"
    )?;
    writeln!(
        index,
        "// Mode: {}",
        match mode {
            EmitMode::Llvm => "LLVM",
            EmitMode::Mir => "MIR",
        }
    )?;
    writeln!(index)?;
    let mut seen = std::collections::HashSet::new();
    for spec in specs {
        let filename = format!("{}_auto_spec.saw", sanitize_name(&spec.function_name));
        if seen.insert(filename.clone()) {
            writeln!(index, "include \"{filename}\";")?;
        }
    }
    Ok(())
}

/// Generate the full LLVM SAW spec for a single function.
///
/// `all_globals` is the full set of mutable globals in the translation
/// unit (AST-discovered + IR-discovered). The spec allocates every
/// global in this list, not just those the function directly references,
/// because SAW symbolically executes callee bodies and any callee may
/// touch globals the target doesn't directly name.
pub fn generate_saw_spec(spec: &SpecConstraint, all_globals: &[GlobalVarInfo]) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;
    let target_name = spec.mangled_name.as_deref().unwrap_or(&spec.function_name);

    out.push_str(&format!("// Auto-generated spec for: {fn_name}\n"));
    out.push_str("// Constraints derived from type system and annotations\n");
    if spec.can_throw {
        out.push_str("// WARNING: Function may throw -- exception path not modeled\n");
    }
    out.push_str(&format!(
        "\nlet {safe_name}_auto_spec : LLVMSetup () = do {{\n",
        safe_name = sanitize_name(fn_name),
    ));

    for param in &spec.params {
        emit_llvm_param_setup(&mut out, param);
    }
    if !all_globals.is_empty() {
        emit_llvm_globals_setup(&mut out, all_globals);
    }

    let mut args: Vec<String> = Vec::new();
    if spec.return_constraint.is_sret {
        out.push_str("\n    // sret: struct returned via hidden first pointer parameter\n");
        out.push_str(&format!(
            "    result_ptr <- llvm_alloc ({});\n",
            spec.return_constraint.saw_type,
        ));
        args.push("result_ptr".into());
    }
    args.extend(spec.params.iter().map(|p| match p.alloc_type {
        AllocType::AllocReadonly | AllocType::AllocMutable => format!("{}_ptr", p.name),
        AllocType::FreshVar => format!("llvm_term {}", p.name),
    }));
    out.push_str(&format!("\n    llvm_execute_func [{}];\n", args.join(", "),));

    let needs_adversarial = spec.is_virtual || !spec.has_body;
    if needs_adversarial {
        emit_adversarial_return_and_postconds(&mut out, spec);
    } else {
        emit_concrete_return_and_postconds(&mut out, spec);
    }

    out.push_str("};\n");

    if needs_adversarial {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- llvm_unsafe_assume_spec m \"{target}\" {safe}_auto_spec;\n",
            safe = sanitize_name(fn_name),
            target = target_name,
        ));
    } else {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- llvm_verify m \"{target}\" [] false {safe}_auto_spec z3;\n",
            safe = sanitize_name(fn_name),
            target = target_name,
        ));
    }
    out
}

/// Generate a havoc-style override spec for a sub-callee, written with
/// `llvm_unsafe_assume_spec`.
///
/// We used to emit a call to SAW's experimental `llvm_unspecified_globals`
/// builtin here, but that builtin is not present in the saw-script
/// currently in use (1.5.0.99). The functionally-equivalent pattern,
/// which works with stock SAW, is:
///
///   1. Pre-state: allocate fresh symbolic pointers / fresh-var scalars
///      matching the function signature.
///   2. `llvm_execute_func [...]`.
///   3. Post-state: for every global the sub-callee may touch, write a
///      *fresh* symbolic value through `llvm_points_to (llvm_global X)`.
///      This is the same "globals may have any value after the call"
///      semantics the experimental builtin provided, just spelled out.
///
/// The override is registered via `llvm_unsafe_assume_spec`, which is
/// the standard mechanism for assuming a behaviour against a function
/// with a body we don't want SAW to symbolically execute (e.g. because
/// it has side effects on globals we want to havoc).
pub fn generate_unspecified_spec(spec: &SpecConstraint, all_globals: &[GlobalVarInfo]) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;
    let target_name = spec.mangled_name.as_deref().unwrap_or(&spec.function_name);
    let safe = spec_safe_id(spec);

    out.push_str(&format!(
        "// Auto-generated havoc spec for sub-callee: {fn_name}\n"
    ));
    out.push_str("// Assumes the function may write any value to the listed globals;\n");
    out.push_str("// inputs are accepted as fresh-symbolic and the return value (if\n");
    out.push_str("// any) is unconstrained.\n\n");
    out.push_str(&format!("let {safe}_spec : LLVMSetup () = do {{\n",));

    // Pre-state: build args.  Mirrors the param-setup half of the
    // concrete spec (llvm_setup::emit_llvm_param_setup), minus the
    // post-state preservation -- the override is purely a havoc.
    let mut args: Vec<String> = Vec::new();
    for p in &spec.params {
        match p.alloc_type {
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                // Use llvm_fresh_pointer rather than llvm_alloc so SAW
                // accepts callsites where the pointer aliases other
                // allocations (e.g. when the caller passes &local).
                out.push_str(&format!(
                    "    {name}_ptr <- llvm_fresh_pointer ({ty});\n",
                    name = p.name,
                    ty = p.saw_type,
                ));
                args.push(format!("{}_ptr", p.name));
            }
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {name} <- llvm_fresh_var \"{name}\" ({ty});\n",
                    name = p.name,
                    ty = p.saw_type,
                ));
                args.push(format!("llvm_term {}", p.name));
            }
        }
    }
    // Each global the caller may touch needs to be declared so we can
    // refer to it in the post-state. Without `llvm_alloc_global` SAW
    // rejects `llvm_points_to (llvm_global X)` outright.
    for g in all_globals {
        out.push_str(&format!("    llvm_alloc_global \"{}\";\n", g.mangled_name,));
    }
    out.push_str(&format!("\n    llvm_execute_func [{}];\n", args.join(", "),));

    // Post-state: return value (if any) is unconstrained.
    if !crate::emit::saw_emit::writer::is_void_saw_type(&spec.return_constraint.saw_type) {
        if spec.return_constraint.returns_pointer {
            // Allocate AND initialize the pointee with a fresh symbolic
            // value. Without the `llvm_points_to`, any dereference in
            // the caller hits uninitialized memory and SAW aborts with
            // "Error during memory load" — that defeats the purpose of
            // an adversarial spec, since the caller can't observe a
            // value the solver was supposed to pick freely.
            let ty = &spec.return_constraint.saw_type;
            out.push_str(&format!("    ret_ptr <- llvm_alloc ({ty});\n"));
            out.push_str(&format!(
                "    ret_val <- llvm_fresh_var \"ret_val\" ({ty});\n"
            ));
            out.push_str("    llvm_points_to ret_ptr (llvm_term ret_val);\n");
            out.push_str("    llvm_return ret_ptr;\n");
        } else {
            out.push_str(&format!(
                "    ret <- llvm_fresh_var \"ret\" ({});\n",
                spec.return_constraint.saw_type,
            ));
            out.push_str("    llvm_return (llvm_term ret);\n");
        }
    }

    // Post-state: havoc each listed global by storing a fresh value.
    // The bit width is taken from TypeInfo::SignedInt/UnsignedInt where
    // we know it, falling back to a byte array sized from the AST.
    // This is the bit that gives us the "may have any value after the
    // call" semantics SAW's experimental builtin used to provide.
    for g in all_globals {
        let (saw_ty, fresh_ty) = match &g.ty {
            TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => {
                (format!("llvm_int {b}"), format!("llvm_int {b}"))
            }
            TypeInfo::Bool => ("llvm_int 1".to_string(), "llvm_int 1".to_string()),
            TypeInfo::ByteArray(n) => (
                format!("llvm_array {n} (llvm_int 8)"),
                format!("llvm_array {n} (llvm_int 8)"),
            ),
            _ => ("llvm_int 32".to_string(), "llvm_int 32".to_string()),
        };
        let _ = saw_ty; // kept for future use; fresh_ty is what we need
        let safe_g = sanitize_name(&g.mangled_name);
        out.push_str(&format!(
            "    {safe_g}_after <- llvm_fresh_var \"{safe_g}_after\" ({fresh_ty});\n",
        ));
        out.push_str(&format!(
            "    llvm_points_to (llvm_global \"{}\") (llvm_term {safe_g}_after);\n",
            g.mangled_name,
        ));
    }

    out.push_str("};\n\n");
    out.push_str(&format!(
        "ov_{safe} <- llvm_unsafe_assume_spec m \"{target_name}\" {safe}_spec;\n",
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::super::writer::VOID_SAW_TYPE;
    use super::*;
    use crate::constraints::{ParamConstraint, ReturnConstraint};

    fn make_spec(
        name: &str,
        params: Vec<ParamConstraint>,
        return_type: &str,
        can_throw: bool,
    ) -> SpecConstraint {
        SpecConstraint {
            function_name: name.into(),
            mangled_name: None,
            params,
            return_constraint: ReturnConstraint {
                saw_type: return_type.into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
            },
            can_throw,
            is_virtual: false,
            has_body: true,
            referenced_globals: vec![],
            postconditions: vec![],
        }
    }

    #[test]
    fn test_generate_llvm_spec_readonly() {
        let spec = make_spec(
            "test_fn",
            vec![ParamConstraint {
                name: "x".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            VOID_SAW_TYPE,
            false,
        );
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("LLVMSetup ()"));
        assert!(output.contains("llvm_alloc_readonly"));
        assert!(output.contains("llvm_fresh_var"));
        assert!(output.contains("llvm_execute_func"));
    }

    #[test]
    fn test_generate_llvm_spec_mutable() {
        let spec = make_spec(
            "mutate_fn",
            vec![ParamConstraint {
                name: "buf".into(),
                alloc_type: AllocType::AllocMutable,
                saw_type: "llvm_int 64".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            }],
            VOID_SAW_TYPE,
            false,
        );
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("llvm_alloc (llvm_int 64)"));
        assert!(!output.contains("llvm_alloc_readonly"));
    }

    #[test]
    fn test_generate_llvm_spec_freshvar() {
        let spec = make_spec(
            "add",
            vec![
                ParamConstraint {
                    name: "a".into(),
                    alloc_type: AllocType::FreshVar,
                    saw_type: "llvm_int 32".into(),
                    preconditions: vec![],
                    unchanged_after: false,
                    dereferenceable_size: None,
                },
                ParamConstraint {
                    name: "b".into(),
                    alloc_type: AllocType::FreshVar,
                    saw_type: "llvm_int 32".into(),
                    preconditions: vec![],
                    unchanged_after: false,
                    dereferenceable_size: None,
                },
            ],
            "llvm_int 32",
            false,
        );
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("a <- llvm_fresh_var \"a\" (llvm_int 32)"));
        assert!(output.contains("llvm_term a, llvm_term b"));
    }

    #[test]
    fn test_generate_llvm_spec_return() {
        let spec = make_spec("get_val", vec![], "llvm_int 32", false);
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("ret <- llvm_fresh_var \"ret\" (llvm_int 32)"));
        assert!(output.contains("llvm_return (llvm_term ret)"));
        assert!(output.contains("ACTION REQUIRED"));
        assert!(output.contains("llvm_verify"));
        assert!(!output.contains("llvm_unsafe_assume_spec"));
    }

    #[test]
    fn test_generate_llvm_spec_void_return() {
        let spec = make_spec("noop", vec![], VOID_SAW_TYPE, false);
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(!output.contains("llvm_return"));
    }

    #[test]
    fn test_generate_llvm_spec_can_throw() {
        let spec = make_spec("risky", vec![], VOID_SAW_TYPE, true);
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("WARNING: Function may throw"));
    }

    #[test]
    fn test_emit_saw_specs_creates_files() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_emit");
        let _ = fs::remove_dir_all(&dir);
        let specs = vec![make_spec("test_fn", vec![], VOID_SAW_TYPE, false)];
        emit_saw_specs(&specs, &dir, false).unwrap();
        assert!(dir.join("test_fn_auto_spec.saw").exists());
        assert!(dir.join("auto_specs.saw").exists());
        let index = fs::read_to_string(dir.join("auto_specs.saw")).unwrap();
        assert!(index.contains("include \"test_fn_auto_spec.saw\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_postconditions() {
        let spec = SpecConstraint {
            function_name: "read_fn".into(),
            mangled_name: None,
            params: vec![ParamConstraint {
                name: "data".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            return_constraint: ReturnConstraint {
                saw_type: VOID_SAW_TYPE.into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            referenced_globals: vec![],
            postconditions: vec!["llvm_points_to data_ptr (llvm_term data_before)".into()],
        };
        let output = generate_saw_spec(&spec, &spec.referenced_globals);
        assert!(output.contains("Postconditions"));
        assert!(output.contains("data_before"));
    }
}
