//! MIR-mode SAW spec emission for Rust verification.
//!
//! Mirrors the LLVM spec shape but uses `mir_*` SAW commands and the
//! Rust ADT type names.

use super::llvm_spec::{emit_saw_specs_with_mode, EmitMode};
use super::names::sanitize_name;
use super::types::{cryptol_type_for_saw, mir_saw_type};
use super::writer::is_void_saw_type;
use crate::constraints::*;
use anyhow::Result;
use std::path::Path;

/// Emit MIR SAW spec files for all constraints.
pub fn emit_mir_saw_specs(specs: &[SpecConstraint], output_dir: &Path) -> Result<()> {
    emit_saw_specs_with_mode(specs, output_dir, EmitMode::Mir, false, &[])
}

/// Generate the full MIR SAW spec for a single function.
pub fn generate_mir_spec(spec: &SpecConstraint) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;

    out.push_str(&format!("// Auto-generated MIR spec for: {fn_name}\n"));
    out.push_str("// Constraints derived from Rust type system\n");
    out.push_str(&format!(
        "\nlet {safe_name}_auto_spec : MIRSetup () = do {{\n",
        safe_name = sanitize_name(fn_name),
    ));

    for param in &spec.params {
        emit_mir_param_setup(&mut out, param);
    }

    let args: Vec<String> = spec
        .params
        .iter()
        .map(|p| match p.alloc_type {
            AllocType::AllocReadonly | AllocType::AllocMutable => format!("{}_ref", p.name),
            AllocType::FreshVar => format!("mir_term {}", p.name),
        })
        .collect();
    out.push_str(&format!("\n    mir_execute_func [{}];\n", args.join(", "),));

    let needs_adversarial = spec.is_virtual || !spec.has_body;
    if needs_adversarial {
        emit_mir_adversarial(&mut out, spec);
    } else {
        emit_mir_concrete(&mut out, spec);
    }

    out.push_str("};\n");

    if needs_adversarial {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- mir_unsafe_assume_spec m \"{target}\" {safe}_auto_spec;\n",
            safe = sanitize_name(fn_name),
            target = fn_name,
        ));
    } else {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- mir_verify m \"{target}\" [] false {safe}_auto_spec z3;\n",
            safe = sanitize_name(fn_name),
            target = fn_name,
        ));
    }
    out
}

fn emit_mir_param_setup(out: &mut String, param: &ParamConstraint) {
    out.push_str(&format!("\n    // Parameter: {}\n", param.name));
    match param.alloc_type {
        AllocType::AllocReadonly => {
            out.push_str(&format!(
                "    {name}_ref <- mir_alloc_readonly (mir_find_adt m \"{ty}\");\n",
                name = param.name,
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    {name}_before <- mir_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = mir_saw_type(&param.saw_type),
            ));
            out.push_str(&format!(
                "    mir_points_to {name}_ref (mir_term {name}_before);\n",
                name = param.name,
            ));
        }
        AllocType::AllocMutable => {
            out.push_str(&format!(
                "    {name}_ref <- mir_alloc (mir_find_adt m \"{ty}\");\n",
                name = param.name,
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    {name}_val <- mir_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = mir_saw_type(&param.saw_type),
            ));
            out.push_str(&format!(
                "    mir_points_to {name}_ref (mir_term {name}_val);\n",
                name = param.name,
            ));
        }
        AllocType::FreshVar => {
            out.push_str(&format!(
                "    {name} <- mir_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = mir_saw_type(&param.saw_type),
            ));
        }
    }
    for pre in &param.preconditions {
        let mir_pre = pre
            .replace("llvm_precond", "mir_precond")
            .replace("llvm_", "mir_");
        out.push_str(&format!("    {mir_pre};\n"));
    }
}

fn emit_mir_adversarial(out: &mut String, spec: &SpecConstraint) {
    if !is_void_saw_type(&spec.return_constraint.saw_type) {
        out.push_str(&format!(
            "\n    ret <- mir_fresh_var \"ret\" ({});\n",
            mir_saw_type(&spec.return_constraint.saw_type),
        ));
        for vc in &spec.return_constraint.value_constraints {
            let mir_vc = vc
                .replace("llvm_postcond", "mir_postcond")
                .replace("llvm_", "mir_");
            out.push_str(&format!("    {mir_vc};\n"));
        }
        out.push_str("    mir_return (mir_term ret);\n");
    }

    out.push_str("\n    // Adversarial postconditions\n");
    for param in &spec.params {
        match param.alloc_type {
            AllocType::AllocReadonly => {
                out.push_str(&format!(
                    "    // {name}: PRESERVED (immutable ref)\n",
                    name = param.name,
                ));
                out.push_str(&format!(
                    "    mir_points_to {name}_ref (mir_term {name}_before);\n",
                    name = param.name,
                ));
            }
            AllocType::AllocMutable => {
                out.push_str(&format!(
                    "    // {name}: HAVOCED (mutable ref)\n",
                    name = param.name,
                ));
                out.push_str(&format!(
                    "    {name}_after <- mir_fresh_var \"{name}_after\" ({ty});\n",
                    name = param.name,
                    ty = mir_saw_type(&param.saw_type),
                ));
                out.push_str(&format!(
                    "    mir_points_to {name}_ref (mir_term {name}_after);\n",
                    name = param.name,
                ));
            }
            AllocType::FreshVar => {}
        }
    }
}

fn emit_mir_concrete(out: &mut String, spec: &SpecConstraint) {
    if !is_void_saw_type(&spec.return_constraint.saw_type) {
        out.push_str(&format!(
            "\n    ret <- mir_fresh_var \"ret\" ({});\n",
            mir_saw_type(&spec.return_constraint.saw_type),
        ));
        for vc in &spec.return_constraint.value_constraints {
            let mir_vc = vc
                .replace("llvm_postcond", "mir_postcond")
                .replace("llvm_", "mir_");
            out.push_str(&format!("    {mir_vc};\n"));
        }
        out.push_str(
            "    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n",
        );
        out.push_str(
            "    // For functional correctness, import your Cryptol spec and replace ret:\n",
        );
        out.push_str(&format!(
            "    //   mir_return (mir_term {{{{ my_spec x : {} }}}});\n",
            cryptol_type_for_saw(&spec.return_constraint.saw_type),
        ));
        out.push_str("    mir_return (mir_term ret);\n");
    }

    if !spec.postconditions.is_empty() {
        out.push_str("\n    // Postconditions\n");
        for post in &spec.postconditions {
            let mir_post = post
                .replace("llvm_points_to", "mir_points_to")
                .replace("llvm_term", "mir_term");
            out.push_str(&format!("    {mir_post};\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{ParamConstraint, ReturnConstraint};
    use std::fs;

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
    fn test_generate_mir_spec_basic() {
        let spec = make_spec(
            "rust_fn",
            vec![ParamConstraint {
                name: "x".into(),
                alloc_type: AllocType::FreshVar,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            }],
            "llvm_int 32",
            false,
        );
        let output = generate_mir_spec(&spec);
        assert!(output.contains("MIRSetup ()"));
        assert!(output.contains("mir_fresh_var"));
        assert!(output.contains("mir_execute_func"));
        assert!(output.contains("mir_return"));
    }

    #[test]
    fn test_generate_mir_spec_alloc() {
        let spec = make_spec(
            "rust_ref_fn",
            vec![ParamConstraint {
                name: "data".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 64".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            "// void",
            false,
        );
        let output = generate_mir_spec(&spec);
        assert!(output.contains("mir_alloc_readonly"));
        assert!(output.contains("mir_points_to"));
    }

    #[test]
    fn test_emit_mir_saw_specs() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_mir_emit");
        let _ = fs::remove_dir_all(&dir);
        let specs = vec![make_spec("rust_fn", vec![], "llvm_int 32", false)];
        emit_mir_saw_specs(&specs, &dir).unwrap();
        let content = fs::read_to_string(dir.join("rust_fn_auto_spec.saw")).unwrap();
        assert!(content.contains("MIRSetup"));
        let index = fs::read_to_string(dir.join("auto_specs.saw")).unwrap();
        assert!(index.contains("Mode: MIR"));
        let _ = fs::remove_dir_all(&dir);
    }
}
