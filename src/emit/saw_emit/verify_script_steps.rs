//! Step emitters used by [`super::verify_script::emit_verification_script`].
//!
//! These helpers were factored out of `verify_script.rs` to keep that
//! file under the 500-non-whitespace-line repository limit. Each
//! function appends its corresponding chunk of SAWScript to the `out`
//! buffer.

use super::cryptol_bridge::{cryptol_arg_for, cryptol_return_for};
use super::names::{sanitize_name, spec_safe_id, stub_function_name};
use super::overrides::{container_layout_for, emit_container_this};
use super::stubs::AssembledStubs;
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use std::collections::HashSet;

pub(super) fn is_operator_new(spec: &SpecConstraint) -> bool {
    spec.function_name == "operator new" || spec.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z")
}

/// Emit the per-parameter `llvm_precond` clauses derived in
/// [`crate::constraints::derive`]. Lines that look like comments are
/// passed through verbatim; everything else gets a trailing `;`.
fn emit_param_preconditions(out: &mut String, preconditions: &[String]) {
    for pre in preconditions {
        let trimmed = pre.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("//") {
            out.push_str(&format!("    {pre}\n"));
        } else {
            out.push_str(&format!("    {pre};\n"));
        }
    }
}

pub(super) fn emit_load_bitcode_step(
    out: &mut String,
    step: &mut u32,
    has_interfaces: bool,
    bitcode_rel: &str,
    stubs_status: &AssembledStubs,
) {
    out.push_str(&format!("// Step {step}: Load bitcode"));
    if has_interfaces {
        // Preferred path: gen-verify already pre-linked main.bc + vtable_stubs.bc
        // into a single combined module via `llvm-link`. This avoids
        // `llvm_combine_modules`, which was added to SAW *after* the v1.5
        // release tag — generating the combine version forces users to
        // build SAW from source.
        if let AssembledStubs::LinkedBitcode {
            combined_filename,
            linker,
        } = stubs_status
        {
            out.push_str(" (vtable stubs pre-linked)\n");
            out.push_str(&format!(
                "// {combined_filename} was produced at gen time via `{linker}`\n"
            ));
            out.push_str("// (main bitcode + vtable_stubs.bc). Loading one module here lets the\n");
            out.push_str("// stock SAW v1.5 release tarball run this script unmodified.\n");
            out.push_str(&format!(
                "m <- llvm_load_module \"{combined_filename}\";\n\n"
            ));
            let _ = step;
            return;
        }
        let stubs_filename = stubs_status.script_filename().unwrap_or("vtable_stubs.bc");
        out.push_str(" + vtable stubs\n");
        if let AssembledStubs::TextOnly { ll_filename } = stubs_status {
            out.push_str("//\n");
            out.push_str("// WARNING: vtable_stubs.bc was NOT auto-assembled at gen time\n");
            out.push_str("//          (llvm-as / clang not found on PATH). Assemble it before\n");
            out.push_str("//          running SAW:\n");
            out.push_str(&format!(
                "//            llvm-as {ll_filename} -o vtable_stubs.bc\n"
            ));
            out.push_str(&format!(
                "//          (or)  clang -c -emit-llvm {ll_filename} -o vtable_stubs.bc\n"
            ));
            out.push_str("//          The load line below already references vtable_stubs.bc.\n");
        } else if let AssembledStubs::Bitcode { assembler, .. } = stubs_status {
            out.push_str(&format!(
                "// vtable_stubs.bc was assembled from vtable_stubs.ll via `{assembler}`.\n",
            ));
            out.push_str(
                "// (Pre-linking with `llvm-link` was disabled via --use-llvm-combine-modules.)\n",
            );
        }
        out.push_str(&format!("m_main  <- llvm_load_module \"{bitcode_rel}\";\n"));
        out.push_str(&format!(
            "m_stubs <- llvm_load_module \"{stubs_filename}\";\n"
        ));
        out.push_str("m       <- llvm_combine_modules m_main [m_stubs];\n\n");
    } else {
        out.push('\n');
        out.push_str(&format!("m <- llvm_load_module \"{bitcode_rel}\";\n\n"));
    }
    let _ = step; // mutation handled by caller
}

/// Emit a no-op `llvm_unsafe_assume_spec` for `llvm.var.annotation.p0.p0`.
///
/// Clang lowers `__attribute__((annotate("...")))` on a local/parameter
/// into a call to this intrinsic in every annotated function. SAW can't
/// simulate it natively (the symbol has no body and no MIR-level
/// equivalent), so without an override SAW aborts the verification with
/// "No implementation or override found for pointer".
///
/// The override accepts five arbitrary parameters (matching the
/// intrinsic's i8* / i8* / i8* / i32 / i8* signature in LLVM 20) and
/// returns void without touching any state. That's sound because the
/// intrinsic itself has no observable effect at runtime -- it carries
/// purely-static metadata for downstream LLVM passes.
pub(super) fn emit_var_annotation_override(
    out: &mut String,
    step: u32,
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!(
        "// Step {step}: No-op override for llvm.var.annotation (SAL/__attribute__((annotate)))\n"
    ));
    out.push_str("let var_annotation_spec = do {\n");
    out.push_str("    p1 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    p2 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    p3 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    line <- llvm_fresh_var \"line\" (llvm_int 32);\n");
    out.push_str("    p4 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    llvm_execute_func [p1, p2, p3, llvm_term line, p4];\n");
    out.push_str("};\n");
    out.push_str(
        "ov_var_annotation <- llvm_unsafe_assume_spec m \"llvm.var.annotation.p0.p0\" var_annotation_spec;\n\n",
    );
    override_names.push("ov_var_annotation".to_string());
}

pub(super) fn emit_external_overrides_step(
    out: &mut String,
    step: u32,
    external_calls: &[SpecConstraint],
    needs_operator_new: bool,
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!("// Step {step}: Assume external functions\n"));
    let mut included: HashSet<String> = HashSet::new();
    for spec in external_calls {
        if is_operator_new(spec) {
            continue;
        }
        let safe_name = spec_safe_id(spec);
        if !included.insert(safe_name.clone()) {
            continue;
        }
        out.push_str(&format!(
            "include \"specs_experimental/{safe_name}_auto_spec.saw\";\n",
        ));
        override_names.push(format!("ov_{safe_name}"));
    }
    if needs_operator_new {
        out.push_str("include \"specs_experimental/operator_new_auto_spec.saw\";\n");
        override_names.push("ov_new".to_string());
    }
}

pub(super) fn emit_sub_callee_step(
    out: &mut String,
    step: u32,
    sub_callee_specs: &[SpecConstraint],
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!(
        "// Step {step}: Sub-function adversarial overrides (compositional)\n",
    ));
    let mut sub_included: HashSet<String> = HashSet::new();
    for spec in sub_callee_specs {
        let safe_name = spec_safe_id(spec);
        if !sub_included.insert(safe_name.clone()) {
            continue;
        }
        out.push_str(&format!(
            "include \"specs_experimental/{safe_name}_auto_spec.saw\";\n",
        ));
        override_names.push(format!("ov_{safe_name}"));
    }
    out.push('\n');
}

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
    all_globals: &[GlobalVarInfo],
) -> (Vec<String>, Vec<String>) {
    out.push_str(&format!(
        "// Step {step}: Equivalence spec — C++ {function_name} == Cryptol {cryptol_fn}\n",
    ));
    out.push_str(&format!("let {function_name}_equiv_spec = do {{\n"));

    let mut cryptol_args = Vec::new();
    let mut execute_args = Vec::new();
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
                emit_param_preconditions(out, &param.preconditions);
                let arg_ty = target_fn
                    .params
                    .get(i)
                    .map(|p| &p.ty)
                    .unwrap_or(&TypeInfo::Void);
                cryptol_args.push(cryptol_arg_for(&param.name, arg_ty));
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
                emit_param_preconditions(out, &param.preconditions);
                let arg_ty = target_fn
                    .params
                    .get(i)
                    .map(|p| &p.ty)
                    .unwrap_or(&TypeInfo::Void);
                cryptol_args.push(cryptol_arg_for(&val_name, arg_ty));
                execute_args.push(ptr_name);
            }
        }
    }

    // sret: when the C++ function returns an aggregate that the MSVC
    // x64 ABI passes via a hidden output pointer, the LLVM signature
    // has `ptr sret(T)` in argument position 0 (free function) or 1
    // (instance method, immediately after `this`). Without inserting
    // that pointer here, SAW rejects the verify with
    // "Type mismatch in argument 0 ... declared with type: ptr, but
    // provided argument has incompatible type: iN". See
    // `SAW_SPEC_GEN_BUG_REPORT_sret_not_detected.md`.
    if target_spec.return_constraint.is_sret {
        out.push_str("\n    // sret: aggregate return passed via hidden output pointer.\n");
        out.push_str(&format!(
            "    result_ptr <- llvm_alloc ({});\n",
            target_spec.return_constraint.saw_type,
        ));
        if target_spec.return_constraint.sret_prestate {
            // The Cryptol model takes the sret buffer's pre-call contents
            // as a trailing parameter. Allocate a fresh symbolic value,
            // bind it to the buffer before llvm_execute_func, and append
            // it to the Cryptol argument list.
            out.push_str(&format!(
                "    result_pre <- llvm_fresh_var \"result_pre\" ({});\n",
                target_spec.return_constraint.saw_type,
            ));
            out.push_str("    llvm_points_to result_ptr (llvm_term result_pre);\n");
            cryptol_args.push("result_pre".to_string());
        }
        let has_this = target_fn
            .params
            .first()
            .map(|p| p.name == "this")
            .unwrap_or(false);
        let insert_idx = if has_this && !execute_args.is_empty() {
            1
        } else {
            0
        };
        execute_args.insert(insert_idx, "result_ptr".to_string());
    }

    out.push('\n');

    for global in all_globals {
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
    return_type: &TypeInfo,
    is_sret: bool,
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
    let cryptol_return = cryptol_return_for(&cryptol_call, return_type);
    let is_void_return = matches!(return_type, TypeInfo::Void);
    if is_sret {
        // Aggregate return via hidden sret pointer: the LLVM function
        // returns void, so `llvm_return` would be a no-op. Bind the
        // Cryptol value into `*result_ptr` via `llvm_points_to`.
        // `result_ptr` was allocated and inserted into the argument
        // list by `emit_equiv_spec_body`.
        if sub_callee_specs.is_empty() {
            out.push_str("    // Postcondition (sret): *result_ptr == Cryptol spec\n");
        } else {
            out.push_str("    // TODO: Compositional sret postcondition — fill in by hand.\n");
            out.push_str("    // The sub-function overrides above return fresh symbolic values;\n");
            out.push_str("    // thread them into the Cryptol call below before relying on\n");
            out.push_str("    // this assertion. The auto-derived call uses only the target's\n");
            out.push_str("    // own parameters and is almost certainly incomplete.\n");
        }
        out.push_str(&format!(
            "    llvm_points_to result_ptr (llvm_term {{{{ {} }}}});\n",
            cryptol_return,
        ));
    } else if is_void_return {
        // Void-returning target: no `llvm_return` to emit. The harness
        // still proves "no UB" (load module + execute), and any
        // `_Out_` parameters require a hand-written
        // `llvm_points_to <ptr> ...` postcondition — the auto-generator
        // does not yet thread output-buffer values back through Cryptol
        // (tracked separately; see tests/e2e/cases/09-type-coverage/void_out_inc).
        out.push_str("    // Void return — no llvm_return to emit.\n");
        out.push_str(&format!(
            "    // (Cryptol spec `{}` is referenced for documentation only.)\n",
            cryptol_return,
        ));
    } else if sub_callee_specs.is_empty() {
        out.push_str("    // Postcondition: C++ result == Cryptol spec\n");
        out.push_str(&format!(
            "    llvm_return (llvm_term {{{{ {} }}}});\n",
            cryptol_return,
        ));
    } else {
        out.push_str("    // TODO: Compositional postcondition — fill in by hand.\n");
        out.push_str("    //\n");
        out.push_str("    // The sub-function overrides above return fresh symbolic values.\n");
        out.push_str("    // To check functional correctness, capture those returns (e.g.\n");
        out.push_str("    //   ret_helper <- llvm_fresh_var \"ret_helper\" (llvm_int 32);\n");
        out.push_str("    // inside the matching override spec) and thread them into the\n");
        out.push_str("    // Cryptol equivalence call below.  The auto-derived call only uses\n");
        out.push_str("    // the target's own parameters and is almost certainly incomplete.\n");
        out.push_str(&format!(
            "    llvm_return (llvm_term {{{{ {} }}}});  // <-- replace with full mapping\n",
            cryptol_return,
        ));
    }
    out.push_str("};\n\n");
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_verify_step(
    out: &mut String,
    step: u32,
    function_name: &str,
    cryptol_fn: &str,
    mangled_name: &str,
    has_interfaces: bool,
    vmethods: &[InterfaceMethod],
    constructors: &[ClassConstructor],
    override_names: Vec<String>,
) {
    out.push_str(&format!("// Step {step}: Verify equivalence\n"));
    out.push_str(&format!(
        "print \"=== Checking: {function_name} == {cryptol_fn} (Cryptol) ===\";\n",
    ));

    let mut overrides = override_names;
    if has_interfaces {
        let ctor_classes: HashSet<&str> =
            constructors.iter().map(|c| c.class_name.as_str()).collect();
        for class in &ctor_classes {
            let safe_class = sanitize_name(class).to_lowercase();
            overrides.push(format!("ov_{safe_class}_ctor"));
        }
        for method in vmethods {
            if method.is_override {
                continue;
            }
            let stub_name = stub_function_name(method);
            let safe_name = sanitize_name(&stub_name);
            overrides.push(format!("ov_{safe_name}"));
        }
    }

    let overrides_str = if overrides.len() <= 4 {
        format!("[{}]", overrides.join(", "))
    } else {
        let mut s = String::from("[\n");
        for (i, ov) in overrides.iter().enumerate() {
            if i == 0 {
                s.push_str(&format!("     {ov}"));
            } else {
                s.push_str(&format!(",\n     {ov}"));
            }
        }
        s.push(']');
        s
    };

    out.push_str(&format!(
        "llvm_verify m \"{mangled_name}\"\n    {overrides_str}\n    false {function_name}_equiv_spec z3;\n\n",
    ));
    out.push_str(&format!(
        "print \"=== VERIFIED: {function_name} == {cryptol_fn} ===\";\n",
    ));
}

#[cfg(test)]
#[path = "verify_script_steps_tests.rs"]
mod tests;
