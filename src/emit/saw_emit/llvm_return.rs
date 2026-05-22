//! Return-value + postcondition emission for the LLVM spec generator.
//!
//! Split into adversarial (virtual / external) vs concrete (real
//! implementation) variants. The two share enough structure that a
//! single dispatch in [`super::llvm_spec`] decides which to call.

use super::types::cryptol_type_for_saw;
use super::writer::is_void_saw_type;
use crate::constraints::*;

/// Adversarial model: solver picks any return value, mutable parameter
/// memory is havoced, const parameter memory is preserved.
pub fn emit_adversarial_return_and_postconds(out: &mut String, spec: &SpecConstraint) {
    if spec.return_constraint.returns_pointer {
        out.push_str(&format!(
            "\n    ret_ptr <- llvm_alloc ({});\n",
            spec.return_constraint.saw_type,
        ));
        out.push_str("    llvm_return ret_ptr;\n");
    } else if spec.return_constraint.is_sret {
        out.push_str(&format!(
            "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
            spec.return_constraint.saw_type,
        ));
        for vc in &spec.return_constraint.value_constraints {
            out.push_str(&format!("    {vc};\n"));
        }
        out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
    } else if !is_void_saw_type(&spec.return_constraint.saw_type) {
        out.push_str(&format!(
            "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
            spec.return_constraint.saw_type,
        ));
        for vc in &spec.return_constraint.value_constraints {
            out.push_str(&format!("    {vc};\n"));
        }
        out.push_str("    llvm_return (llvm_term ret);\n");
    }

    out.push_str("\n    // Adversarial postconditions\n");
    for param in &spec.params {
        match param.alloc_type {
            AllocType::AllocReadonly => {
                out.push_str(&format!(
                    "    // {name}: PRESERVED (const)\n",
                    name = param.name,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {name}_ptr (llvm_term {name}_before);\n",
                    name = param.name,
                ));
            }
            AllocType::AllocMutable => {
                out.push_str(&format!(
                    "    // {name}: HAVOCED (mutable)\n",
                    name = param.name,
                ));
                out.push_str(&format!(
                    "    {name}_after <- llvm_fresh_var \"{name}_after\" ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {name}_ptr (llvm_term {name}_after);\n",
                    name = param.name,
                ));
            }
            AllocType::FreshVar => { /* pass-by-value: nothing to havoc */ }
        }
    }
}

/// Concrete model: the spec drives `llvm_verify` against the real
/// implementation. Emits an unconstrained return + an ACTION REQUIRED
/// comment showing how to swap in a Cryptol postcondition.
pub fn emit_concrete_return_and_postconds(out: &mut String, spec: &SpecConstraint) {
    if spec.return_constraint.returns_pointer {
        out.push_str(&format!(
            "\n    ret_ptr <- llvm_alloc ({});\n",
            spec.return_constraint.saw_type,
        ));
        out.push_str("    llvm_return ret_ptr;\n");
    } else if spec.return_constraint.is_sret {
        out.push_str(&format!(
            "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
            spec.return_constraint.saw_type,
        ));
        for vc in &spec.return_constraint.value_constraints {
            out.push_str(&format!("    {vc};\n"));
        }
        out.push_str(
            "    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n",
        );
        out.push_str(
            "    // For functional correctness, import your Cryptol spec and replace ret:\n",
        );
        out.push_str("    //   import \"my_spec.cry\";\n");
        out.push_str("    //   llvm_points_to result_ptr (llvm_term {{ my_spec x }});\n");
        out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
    } else if !is_void_saw_type(&spec.return_constraint.saw_type) {
        out.push_str(&format!(
            "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
            spec.return_constraint.saw_type,
        ));
        for vc in &spec.return_constraint.value_constraints {
            out.push_str(&format!("    {vc};\n"));
        }
        out.push_str(
            "    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n",
        );
        out.push_str(
            "    // For functional correctness, import your Cryptol spec and replace ret:\n",
        );
        out.push_str("    //   import \"my_spec.cry\";\n");
        out.push_str(&format!(
            "    //   llvm_return (llvm_term {{{{ my_spec x : {} }}}});\n",
            cryptol_type_for_saw(&spec.return_constraint.saw_type),
        ));
        out.push_str("    llvm_return (llvm_term ret);\n");
    }

    if !spec.postconditions.is_empty() {
        out.push_str("\n    // Postconditions\n");
        for post in &spec.postconditions {
            out.push_str(&format!("    {post};\n"));
        }
    }
}
