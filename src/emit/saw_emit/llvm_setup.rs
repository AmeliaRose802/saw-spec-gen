//! Per-parameter setup blocks shared by the LLVM spec emitter.

use super::names::object_allocator;
use super::names::sanitize_name;
use crate::constraints::*;

/// Emit the parameter-setup block (alloc + fresh var + points_to) plus
/// any explicit preconditions for a single parameter.
pub fn emit_llvm_param_setup(out: &mut String, param: &ParamConstraint) {
    out.push_str(&format!("\n    // Parameter: {}\n", param.name));
    match param.alloc_type {
        AllocType::AllocReadonly => {
            out.push_str(&format!(
                "    {name}_ptr <- {alloc} ({ty});\n",
                name = param.name,
                alloc = object_allocator(false, &param.saw_type),
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    {name}_before <- llvm_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    llvm_points_to {name}_ptr (llvm_term {name}_before);\n",
                name = param.name,
            ));
        }
        AllocType::AllocMutable => {
            out.push_str(&format!(
                "    {name}_ptr <- {alloc} ({ty});\n",
                name = param.name,
                alloc = object_allocator(true, &param.saw_type),
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    {name}_val <- llvm_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = param.saw_type,
            ));
            out.push_str(&format!(
                "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n",
                name = param.name,
            ));
        }
        AllocType::FreshVar => {
            out.push_str(&format!(
                "    {name} <- llvm_fresh_var \"{name}\" ({ty});\n",
                name = param.name,
                ty = param.saw_type,
            ));
        }
    }
    for pre in &param.preconditions {
        out.push_str(&format!("    {pre};\n"));
    }
}

/// Emit the global-variable setup block (`llvm_alloc_global` + initial
/// value or fresh var).
pub fn emit_llvm_globals_setup(out: &mut String, globals: &[GlobalVarInfo]) {
    out.push_str("\n    // Referenced global variables (initialized to compile-time values)\n");
    for global in globals {
        let safe_name = sanitize_name(&global.name);
        let saw_type = type_to_saw(&global.ty);
        out.push_str(&format!(
            "    llvm_alloc_global \"{}\";\n",
            global.mangled_name,
        ));
        if let Some(init_val) = &global.init_value {
            let bits = match &global.ty {
                TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => *b,
                TypeInfo::Bool => 1,
                _ => 32,
            };
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {{{{ {} : [{}] }}}});\n",
                global.mangled_name, init_val, bits,
            ));
        } else if global.has_static_initializer {
            // Aggregate global with a compile-time initializer we can't
            // render as a scalar (e.g. `static std::mutex g_mtx;`) — seed
            // its pre-state from the module's own static initializer.
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{name}\") (llvm_global_initializer \"{name}\");\n",
                name = global.mangled_name,
            ));
        } else {
            out.push_str(&format!(
                "    {safe_name}_global <- llvm_fresh_var \"{safe_name}\" ({saw_type});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {safe_name}_global);\n",
                global.mangled_name,
            ));
        }
    }
}
