//! Complete end-to-end verification script generation (`verify.saw`).
//!
//! Coordinates everything else: loads bitcode, wires vtable overrides,
//! imports the Cryptol spec, builds the equivalence body, and invokes
//! `llvm_verify`.

use super::bitcode_overrides::EmittedBitcodeOverrides;
use super::path_utils::pathdiff_or_absolute;
use super::stubs::AssembledStubs;
use super::verify_script_sret::{detect_sret_prestate, emit_header, make_interface_of};
use super::verify_script_steps::{
    emit_equiv_spec_body, emit_external_overrides_step, emit_load_bitcode_step,
    emit_postcondition_and_close, emit_sub_callee_step, emit_var_annotation_override,
    emit_verify_step, is_operator_new, InterfaceCtx, PostconditionCtx,
};
use crate::buffer_overrides::BufferOverrides;
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use crate::llvm_ir::IrStructDef;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Emit a complete, runnable SAW verification script that checks a C++
/// function against a Cryptol spec.
#[allow(clippy::too_many_arguments)]
pub fn emit_verification_script(
    bitcode_path: &Path,
    cryptol_spec_path: &Path,
    cryptol_fn: &str,
    function_name: &str,
    mangled_name: &str,
    target_spec: &SpecConstraint,
    target_fn: &FunctionInfo,
    vmethods: &[InterfaceMethod],
    has_interfaces: bool,
    external_calls: &[SpecConstraint],
    sub_callee_specs: &[SpecConstraint],
    all_globals: &[GlobalVarInfo],
    constructors: &[ClassConstructor],
    stubs_status: &AssembledStubs,
    bitcode_overrides: &EmittedBitcodeOverrides,
    ir_struct_defs: &HashMap<String, IrStructDef>,
    output_dir: &Path,
    buffer_overrides: &BufferOverrides,
    has_source_sal_annotations: bool,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let interface_classes: HashSet<String> =
        vmethods.iter().map(|m| m.class_name.clone()).collect();
    let interface_of = make_interface_of(&interface_classes);

    let bitcode_rel = pathdiff_or_absolute(bitcode_path, output_dir);
    let cryptol_rel = pathdiff_or_absolute(cryptol_spec_path, output_dir);

    let needs_operator_new = external_calls.iter().any(is_operator_new);
    let needs_experimental = !external_calls.is_empty()
        || has_interfaces
        || !sub_callee_specs.is_empty()
        || !bitcode_overrides.is_empty();

    let mut out = String::new();
    let mut step = 1u32;

    emit_header(
        &mut out,
        function_name,
        mangled_name,
        &cryptol_rel,
        cryptol_fn,
        needs_experimental,
    );

    emit_load_bitcode_step(
        &mut out,
        &mut step,
        has_interfaces,
        &bitcode_rel,
        stubs_status,
    );

    if has_interfaces {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Vtable havoc overrides + constructor overrides\n"
        ));
        out.push_str(&format!("import \"{}\";\n", cryptol_rel));
        out.push_str("include \"interface_overrides.saw\";\n\n");
    }

    let mut override_names: Vec<String> = Vec::new();

    // Inject a no-op override for `@llvm.var.annotation` intrinsics generated
    // by SAL annotations on parameters (SAW can't simulate them). Only emit
    // when the source actually had SAL annotations — synthetic annotations
    // added by gen-verify passes (length-binding, struct-shape) do NOT
    // produce this intrinsic in the bitcode.
    let needs_var_annotation_ov = has_source_sal_annotations;
    if needs_var_annotation_ov {
        step += 1;
        emit_var_annotation_override(&mut out, step, &mut override_names);
    }

    if !external_calls.is_empty() || needs_operator_new {
        step += 1;
        emit_external_overrides_step(
            &mut out,
            step,
            external_calls,
            needs_operator_new,
            &mut override_names,
        );
    }

    // Bitcode-derived overrides for symbols the AST path filter dropped
    // (notably `printf` and other system-header inline-defined functions)
    // and for variadic functions whose body uses `llvm.va_*` intrinsics
    // SAW cannot symbolically execute. See
    // `src/transform/extern_override_scan.rs` for the analysis and
    // `src/emit/saw_emit/bitcode_overrides.rs` for the emission contract.
    if !bitcode_overrides.is_empty() {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Bitcode-derived extern overrides\n",
        ));
        out.push_str(&bitcode_overrides.snippet);
        out.push('\n');
        override_names.extend(bitcode_overrides.override_names.iter().cloned());
    }

    if !sub_callee_specs.is_empty() {
        step += 1;
        emit_sub_callee_step(&mut out, step, sub_callee_specs, &mut override_names);
    }

    if !has_interfaces {
        step += 1;
        out.push_str(&format!("// Step {step}: Import Cryptol spec\n"));
        out.push_str(&format!("import \"{}\";\n\n", cryptol_rel));
    }

    // Detect sret prestate threading: if the Cryptol function has one
    // extra trailing [N][8] parameter compared to the C++ user-arg
    // count, the model expects the sret buffer's pre-call bytes.
    let sret_prestate = if target_spec.return_constraint.is_sret {
        detect_sret_prestate(
            cryptol_spec_path,
            cryptol_fn,
            target_spec,
            target_fn,
            ir_struct_defs,
        )
    } else {
        None
    };

    step += 1;
    let (cryptol_args, execute_args) = emit_equiv_spec_body(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        target_spec,
        target_fn,
        &interface_classes,
        &interface_of,
        all_globals,
        sret_prestate.as_ref(),
        buffer_overrides,
    );

    let post_ctx = PostconditionCtx {
        sub_callee_specs,
        return_type: &target_fn.return_type,
        is_sret: target_spec.return_constraint.is_sret,
        return_bridge: None,
    };
    emit_postcondition_and_close(
        &mut out,
        cryptol_fn,
        &cryptol_args,
        &execute_args,
        &post_ctx,
        buffer_overrides,
    );

    let iface_ctx = InterfaceCtx {
        has_interfaces,
        vmethods,
        constructors,
    };
    step += 1;
    emit_verify_step(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        mangled_name,
        &iface_ctx,
        override_names,
    );

    let script_path = output_dir.join("verify.saw");
    fs::write(&script_path, &out)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    Ok(())
}
