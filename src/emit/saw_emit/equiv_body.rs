//! Equivalence-spec body + postcondition emission.
//!
//! Split out of [`super::verify_script`] so that file stays under the
//! 500-line module limit. Both functions are private to the
//! `saw_emit` module.

use super::names::sanitize_name;
use super::overrides::{container_layout_for, emit_container_this};
use crate::constraints::*;
use std::collections::HashSet;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_equiv_spec_body(
    out: &mut String,
    step: u32,
    function_name: &str,
    cryptol_fn: &str,
    target_spec: &SpecConstraint,
    target_fn: &FunctionInfo,
    interface_classes: &HashSet<String>,
    interface_of: &dyn Fn(&TypeInfo) -> Option<String>,
    _all_globals: &[GlobalVarInfo],
) -> (Vec<String>, Vec<String>) {
    out.push_str(&format!(
        "// Step {step}: Equivalence spec — C++ {function_name} == Cryptol {cryptol_fn}\n",
    ));
    out.push_str(&format!("let {function_name}_equiv_spec = do {{\n"));

    let mut cryptol_args = Vec::new();
    let mut execute_args = Vec::new();

    // MSVC x64 ABI: aggregate returns are lowered to a hidden first
    // pointer parameter (`sret`).  Allocate the result buffer up
    // front and prepend it to the argument list; the postcondition
    // emitter will then constrain its contents via `llvm_points_to`
    // instead of `llvm_return`.
    if target_spec.return_constraint.is_sret {
        out.push_str(
            "    // sret: aggregate return passed via hidden first pointer parameter\n",
        );
        out.push_str(&format!(
            "    result_ptr <- llvm_alloc ({});\n",
            target_spec.return_constraint.saw_type,
        ));
        execute_args.push("result_ptr".to_string());
    }

    for (i, param) in target_spec.params.iter().enumerate() {
        let iface = target_fn.params.get(i).and_then(|p| interface_of(&p.ty));
        if let Some(iface_name) = iface {
            let safe_iface = sanitize_name(&iface_name).to_lowercase();
            let ptr_name = format!("{}_ptr", param.name);
            out.push_str(&format!(
                "    // {} : {}* — wire vptr to stub vtable\n",
                param.name, iface_name,
            ));
            out.push_str(&format!("    {ptr_name} <- alloc_{safe_iface}_this;\n"));
            execute_args.push(ptr_name);
            continue;
        }
        if let Some(container) = target_fn
            .params
            .get(i)
            .and_then(|p| container_layout_for(&p.ty, interface_classes))
        {
            emit_container_this(out, &param.name, &container);
            execute_args.push(format!("{}_ptr", param.name));
            continue;
        }
        match param.alloc_type {
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.name, param.saw_type,
                ));
                cryptol_args.push(param.name.clone());
                execute_args.push(format!("llvm_term {}", param.name));
            }
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                let alloc_fn = if param.alloc_type == AllocType::AllocReadonly {
                    "llvm_alloc_readonly"
                } else {
                    "llvm_alloc"
                };
                let ptr_name = format!("{}_ptr", param.name);
                let val_name = param.name.clone();
                out.push_str(&format!(
                    "    {ptr_name} <- {alloc_fn} ({});\n",
                    param.saw_type,
                ));
                out.push_str(&format!(
                    "    {val_name} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {ptr_name} (llvm_term {val_name});\n",
                ));
                cryptol_args.push(val_name);
                execute_args.push(ptr_name);
            }
        }
    }
    out.push('\n');

    for global in &target_spec.referenced_globals {
        out.push_str(&format!(
            "    llvm_alloc_global \"{}\";\n",
            global.mangled_name,
        ));
        if let Some(init) = &global.init_value {
            let bits = match &global.ty {
                TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => *b,
                TypeInfo::Bool => 1,
                _ => 32,
            };
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {{{{ {} : [{}] }}}});\n",
                global.mangled_name, init, bits,
            ));
        }
        out.push('\n');
    }

    (cryptol_args, execute_args)
}

pub(super) fn emit_postcondition_and_close(
    out: &mut String,
    cryptol_fn: &str,
    cryptol_args: &[String],
    execute_args: &[String],
    sub_callee_specs: &[SpecConstraint],
    target_spec: &SpecConstraint,
) {
    out.push_str(&format!(
        "    llvm_execute_func [{}];\n\n",
        execute_args.join(", "),
    ));
    let cryptol_call = if cryptol_args.is_empty() {
        cryptol_fn.to_string()
    } else {
        format!("{} {}", cryptol_fn, cryptol_args.join(" "))
    };
    let is_sret = target_spec.return_constraint.is_sret;
    if sub_callee_specs.is_empty() {
        out.push_str("    // Postcondition: C++ result == Cryptol spec\n");
        if is_sret {
            out.push_str(&format!(
                "    llvm_points_to result_ptr (llvm_term {{{{ {} }}}});\n",
                cryptol_call,
            ));
        } else {
            out.push_str(&format!(
                "    llvm_return (llvm_term {{{{ {} }}}});\n",
                cryptol_call,
            ));
        }
    } else {
        out.push_str("    // TODO: Compositional postcondition — fill in by hand.\n");
        out.push_str("    //\n");
        out.push_str("    // The sub-function overrides above return fresh symbolic values.\n");
        out.push_str("    // To check functional correctness, capture those returns (e.g.\n");
        out.push_str("    //   ret_helper <- llvm_fresh_var \"ret_helper\" (llvm_int 32);\n");
        out.push_str("    // inside the matching override spec) and thread them into the\n");
        out.push_str("    // Cryptol equivalence call below.  The auto-derived call only uses\n");
        out.push_str("    // the target's own parameters and is almost certainly incomplete.\n");
        if is_sret {
            out.push_str(&format!(
                "    llvm_points_to result_ptr (llvm_term {{{{ {} }}}});  // <-- replace with full mapping\n",
                cryptol_call,
            ));
        } else {
            out.push_str(&format!(
                "    llvm_return (llvm_term {{{{ {} }}}});  // <-- replace with full mapping\n",
                cryptol_call,
            ));
        }
    }
    out.push_str("};\n\n");
}
