//! Factory-spec emission for functions that return an interface
//! pointer wired to a stub vtable.
//!
//! Used when the function under verification creates an interface
//! object internally (e.g. a `MakeKeyStore()` factory). The override
//! ensures the returned `this` pointer dispatches to the generated
//! stub vtable rather than the (compiler-emitted) concrete vtable.

use super::names::{sanitize_name, spec_safe_id};
use crate::constraints::*;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Emit a factory spec that returns an `<interface_class>*` wired to the
/// `<interface>_vtable` global produced by [`super::stubs::emit_interface_stubs`].
///
/// Must be included AFTER `interface_overrides.saw` so the
/// `alloc_<iface>_this` helpers it calls are in scope.
pub fn emit_interface_factory_spec(
    spec: &SpecConstraint,
    interface_class: &str,
    all_globals: &[GlobalVarInfo],
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let override_id = spec_safe_id(spec);
    let interface_id = sanitize_name(interface_class).to_lowercase();
    let target_symbol = spec
        .mangled_name
        .as_deref()
        .unwrap_or(&spec.function_name);
    let filepath = output_dir.join(format!("{}_auto_spec.saw", override_id));

    let mut out = String::new();
    out.push_str(&format!(
        "// Factory spec for {}: returns {interface_class}* wired to {interface_id}_vtable.\n",
        spec.function_name,
    ));
    out.push_str("// Must be included after interface_overrides.saw.\n\n");

    out.push_str(&format!("let {override_id}_factory : LLVMSetup () = do {{\n"));

    let mut execute_arguments: Vec<String> = Vec::new();
    for param in &spec.params {
        match param.alloc_type {
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.name, param.saw_type,
                ));
                execute_arguments.push(format!("llvm_term {}", param.name));
            }
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                let allocator = if param.alloc_type == AllocType::AllocReadonly {
                    "llvm_alloc_readonly"
                } else {
                    "llvm_alloc"
                };
                let pointer_name = format!("{}_ptr", param.name);
                out.push_str(&format!(
                    "    {pointer_name} <- {allocator} ({});\n",
                    param.saw_type,
                ));
                execute_arguments.push(pointer_name);
            }
        }
    }

    out.push_str(&format!(
        "\n    llvm_execute_func [{}];\n\n",
        execute_arguments.join(", "),
    ));

    if !all_globals.is_empty() {
        for global in all_globals {
            let width_bits = match &global.ty {
                TypeInfo::SignedInt(width) | TypeInfo::UnsignedInt(width) => *width,
                TypeInfo::Bool => 1,
                _ => 32,
            };
            let post_name = format!("{}_post", sanitize_name(&global.name));
            out.push_str(&format!(
                "    {post_name} <- llvm_fresh_var \"{post_name}\" (llvm_int {width_bits});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {post_name});\n",
                global.mangled_name,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!("    this_pointer <- alloc_{interface_id}_this;\n"));
    out.push_str("    llvm_return this_pointer;\n");
    out.push_str("};\n");

    out.push_str(&format!(
        "ov_{override_id} <- llvm_unsafe_assume_spec m \"{target_symbol}\" {override_id}_factory;\n",
    ));

    fs::write(&filepath, out)?;
    Ok(())
}
