//! Complete end-to-end verification script generation (`verify.saw`).
//!
//! Coordinates everything else: loads bitcode, wires vtable overrides,
//! imports the Cryptol spec, builds the equivalence body, and invokes
//! `llvm_verify`.

use super::path_utils::pathdiff_or_absolute;
use super::stubs::AssembledStubs;
use super::verify_script_steps::{
    emit_equiv_spec_body, emit_external_overrides_step, emit_load_bitcode_step,
    emit_postcondition_and_close, emit_sub_callee_step, emit_var_annotation_override,
    emit_verify_step, is_operator_new,
};
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use anyhow::{Context, Result};
use std::collections::HashSet;
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
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let interface_classes: HashSet<String> =
        vmethods.iter().map(|m| m.class_name.clone()).collect();
    let interface_of = make_interface_of(&interface_classes);

    let bitcode_rel = pathdiff_or_absolute(bitcode_path, output_dir);
    let cryptol_rel = pathdiff_or_absolute(cryptol_spec_path, output_dir);

    let needs_operator_new = external_calls.iter().any(is_operator_new);
    let needs_experimental =
        !external_calls.is_empty() || has_interfaces || !sub_callee_specs.is_empty();

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

    // `__attribute__((annotate("..."))` on a parameter (used by the SAL
    // shim under demos/) compiles into a `call @llvm.var.annotation` in
    // every callsite's prelude. SAW can't simulate that intrinsic and
    // bails out with "No implementation or override found for pointer".
    // Inject a no-op assume-spec override whenever the target function
    // carries SAL annotations on any parameter, so the verifier can
    // silently pass through them.
    let needs_var_annotation_ov = target_fn.params.iter().any(|p| !p.annotations.is_empty());
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

    if !sub_callee_specs.is_empty() {
        step += 1;
        emit_sub_callee_step(&mut out, step, sub_callee_specs, &mut override_names);
    }

    if !has_interfaces {
        step += 1;
        out.push_str(&format!("// Step {step}: Import Cryptol spec\n"));
        out.push_str(&format!("import \"{}\";\n\n", cryptol_rel));
    }

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
    );

    emit_postcondition_and_close(
        &mut out,
        cryptol_fn,
        &cryptol_args,
        &execute_args,
        sub_callee_specs,
    );

    step += 1;
    emit_verify_step(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        mangled_name,
        has_interfaces,
        vmethods,
        constructors,
        override_names,
    );

    let script_path = output_dir.join("verify.saw");
    fs::write(&script_path, &out)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    let _ = all_globals; // silence the lint; consumed indirectly via target_spec
    Ok(())
}

/// Build a closure that maps `TypeInfo` to a known interface class name.
fn make_interface_of(
    interface_classes: &HashSet<String>,
) -> impl Fn(&TypeInfo) -> Option<String> + '_ {
    move |ty: &TypeInfo| {
        if let TypeInfo::Pointer(inner) = ty {
            let name_opt = match inner.as_ref() {
                TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => {
                    Some(name.as_str())
                }
                _ => None,
            };
            if let Some(name) = name_opt {
                if interface_classes.contains(name) {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

fn emit_header(
    out: &mut String,
    function_name: &str,
    mangled_name: &str,
    cryptol_rel: &str,
    cryptol_fn: &str,
    needs_experimental: bool,
) {
    out.push_str(&format!(
        "// Auto-generated SAW verification script\n\
         // Function: {function_name} ({mangled_name})\n\
         // Cryptol spec: {cryptol_rel} :: {cryptol_fn}\n\
         // Generated by saw-spec-gen gen-verify\n\n",
    ));
    if needs_experimental {
        out.push_str("enable_experimental;\n\n");
    }
}
