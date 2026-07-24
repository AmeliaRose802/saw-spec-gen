//! Step emitters used by [`super::verify_script::emit_verification_script`].
//!
//! These helpers were factored out of `verify_script.rs` to keep that
//! file under the 500-non-whitespace-line repository limit. Each
//! function appends its corresponding chunk of SAWScript to the `out`
//! buffer.

use super::cryptol_bridge::cryptol_arg_for;
use super::names::{object_allocator, sanitize_name, spec_safe_id};
use super::overrides::{container_layout_for, emit_container_this};
use super::stubs::AssembledStubs;
use super::verify_script_precond::{emit_param_preconditions, emit_param_preconditions_filtered};
use super::verify_script_sret::SretPrestate;
use crate::buffer_overrides::BufferOverrides;
use crate::constraints::*;
use std::collections::HashSet;

pub(super) fn is_operator_new(spec: &SpecConstraint) -> bool {
    spec.function_name == "operator new" || spec.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z")
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
        // `memcmp` is deferred to the bitcode extern-override scan,
        // which emits a faithful fixed-length spec (or a correctly
        // pointer-typed havoc) instead of this auto-spec's `llvm_int
        // 64`-modeled pointer args, which fail SAW structural matching.
        if spec.function_name == "memcmp" || spec.mangled_name.as_deref() == Some("memcmp") {
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
    sret_prestate: Option<&SretPrestate>,
    buffer_overrides: &BufferOverrides,
) -> (Vec<String>, Vec<String>) {
    out.push_str(&format!(
        "// Step {step}: Equivalence spec — C++ {function_name} == Cryptol {cryptol_fn}\n",
    ));
    out.push_str(&format!("let {function_name}_equiv_spec = do {{\n"));

    let mut cryptol_args = Vec::new();
    let mut execute_args = Vec::new();
    // Cross-parameter preconditions (e.g. `llvm_precond {{ (len : [64]) <= N }}` emitted
    // for `InReadsParam`-annotated buffer parameters) reference sibling length variables
    // that are declared later in the parameter loop.  Collect them here and emit after all
    // parameter declarations so every referenced variable is in scope.
    let mut deferred_preconditions: Vec<(Vec<String>, Option<String>)> = Vec::new();
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

        // CLI buffer-shape overrides (--in-buffer-size / --out-buffer-param):
        // upgrade an auto-inferred 1-byte pointer alloc (or a scalar)
        // into a typed byte-array buffer of the user-declared size.
        // This is what makes pointer-to-buffer C++ APIs verifiable
        // without a hand-written SAW spec.
        let is_in_buf = buffer_overrides.has_in_buffer_size(&param.name);
        let is_out_buf = buffer_overrides.is_out_buffer(&param.name);
        if is_in_buf || is_out_buf {
            let saw_ty = buffer_overrides
                .override_saw_type(&param.name)
                .unwrap_or_else(|| param.saw_type.clone());
            let alloc_fn = object_allocator(is_out_buf, &saw_ty);
            let ptr_name = format!("{}_ptr", param.name);
            // For out-buffers, the value var captures the pre-call
            // contents — rename to `_pre` so its later use in the
            // postcondition reads as "pre-state", not "final state".
            let val_name = buffer_overrides.value_var_for(&param.name);
            out.push_str(&format!("    {ptr_name} <- {alloc_fn} ({saw_ty});\n"));
            out.push_str(&format!(
                "    {val_name} <- llvm_fresh_var \"{val_name}\" ({saw_ty});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to {ptr_name} (llvm_term {val_name});\n",
            ));
            emit_param_preconditions_filtered(out, &param.preconditions, &param.name);

            deferred_preconditions.push((param.preconditions.clone(), Some(param.name.clone())));

            // Push the raw value name as the default Cryptol arg.
            // `--cryptol-arg-order` (consumed in
            // `emit_postcondition_and_close`) overrides this
            // wholesale when the model takes a different shape.
            cryptol_args.push(val_name);
            execute_args.push(ptr_name);
            continue;
        }

        match param.alloc_type {
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.name, param.saw_type,
                ));
                deferred_preconditions.push((param.preconditions.clone(), None));
                let arg_ty = target_fn
                    .params
                    .get(i)
                    .map(|p| &p.ty)
                    .unwrap_or(&TypeInfo::Void);
                cryptol_args.push(cryptol_arg_for(&param.name, arg_ty));
                execute_args.push(format!("llvm_term {}", param.name));
            }
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                let alloc_fn =
                    object_allocator(param.alloc_type == AllocType::AllocMutable, &param.saw_type);
                let ptr_name = format!("{}_ptr", param.name);
                // When the param has an auto-detected output postcondition,
                // use `<name>_pre` for the pre-call value variable so the
                // postcondition can distinguish pre/post state.
                let val_name = if param.out_postcond.is_some() {
                    format!("{}_pre", param.name)
                } else {
                    param.name.clone()
                };
                out.push_str(&format!(
                    "    {ptr_name} <- {alloc_fn} ({});\n",
                    param.saw_type,
                ));
                out.push_str(&format!(
                    "    {val_name} <- llvm_fresh_var \"{}\" ({});\n",
                    val_name, param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {ptr_name} (llvm_term {val_name});\n",
                ));
                deferred_preconditions.push((param.preconditions.clone(), None));
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

    // Emit all parameter preconditions after the full parameter declaration
    // block so that cross-parameter references (e.g. `llvm_precond
    // {{ (len : [64]) <= N }}` emitted for a buffer's `InReadsParam`
    // annotation) are always in scope when evaluated by SAW.
    for (preconditions, filter_name) in &deferred_preconditions {
        if let Some(name) = filter_name {
            emit_param_preconditions_filtered(out, preconditions, name);
        } else {
            emit_param_preconditions(out, preconditions);
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
        if target_spec.return_constraint.sret_prestate && sret_prestate.is_none() {
            // The Cryptol model takes the sret buffer's pre-call contents
            // as a trailing parameter. Allocate a fresh symbolic value,
            // bind it to the buffer before llvm_execute_func, and append
            // it to the Cryptol argument list.
            //
            // Skipped when the caller supplies a SretPrestate struct —
            // that path (below) supersedes this one with take/drop slicing.
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

        // sret prestate threading: when the Cryptol model has an extra
        // trailing [K][8] parameter, it expects (a slice of) the pre-call
        // bytes of the sret buffer so the post-state can refer to them
        // (e.g. for std::optional where the payload is left uninitialised
        // on the nullopt path).
        if let Some(pre) = sret_prestate {
            // When slicing with take/drop, preBytes must be a byte array
            // so Cryptol can treat it as [N][8]. When passing the full
            // buffer, the original struct alias works fine.
            let pre_saw_type = if pre.drop_bytes > 0 && pre.buf_size > 0 {
                format!("llvm_array {} (llvm_int 8)", pre.buf_size)
            } else {
                target_spec.return_constraint.saw_type.clone()
            };
            out.push_str(&format!(
                "    preBytes <- llvm_fresh_var \"preBytes\" ({});\n",
                pre_saw_type,
            ));
            out.push_str("    llvm_points_to result_ptr (llvm_term preBytes);\n");
            #[allow(clippy::unnecessary_map_or)]
            if pre.drop_bytes == 0
                && parse_array_len(&target_spec.return_constraint.saw_type)
                    .map_or(true, |n| pre.take_bytes >= n)
            {
                // Full buffer — pass preBytes directly.
                cryptol_args.push("preBytes".to_string());
            } else {
                // Slice — extract the field sub-range for Cryptol.
                cryptol_args.push(format!(
                    "(take`{{{}}} (drop`{{{}}} preBytes))",
                    pre.take_bytes, pre.drop_bytes,
                ));
            }
        }
    }

    out.push('\n');

    // --max-len-precond bounds: constrain symbolic length params to
    // the declared buffer sizes so SAW can reason about bounded
    // write loops without exhausting the symbolic execution budget.
    // Placed after globals (so all symbolic vars are in scope) and
    // before `llvm_execute_func` (emitted in `emit_postcondition_and_close`).
    for (name, val) in &buffer_overrides.max_len_preconds {
        out.push_str(&format!("    llvm_precond {{{{ `{val} >= {name} }}}};\n",));
    }
    if !buffer_overrides.max_len_preconds.is_empty() {
        out.push('\n');
    }

    if let Some(pre_fn) = buffer_overrides.cryptol_fn_pre() {
        let args = buffer_overrides
            .cryptol_call_args(pre_fn)
            .unwrap_or_else(|| cryptol_args.clone());
        let call = if args.is_empty() {
            pre_fn.to_string()
        } else {
            format!("{} {}", pre_fn, args.join(" "))
        };
        out.push_str(&format!("    llvm_precond {{{{ {call} }}}};\n\n"));
    }

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
        } else if global.has_static_initializer {
            // Aggregate globals (e.g. `static std::mutex g_mtx;`) carry
            // a compile-time initializer we can't render as a scalar.
            // Seed the pre-state with the module's own static
            // initializer so reads of internal fields (mutex recursion
            // count, etc.) see their real freshly-constructed values.
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{name}\") (llvm_global_initializer \"{name}\");\n",
                name = global.mangled_name,
            ));
        }
        out.push('\n');
    }

    (cryptol_args, execute_args)
}

// Re-exported from the extracted `verify_script_close` sibling module
// so that existing `use super::verify_script_steps::{…}` import sites
// and the tests (`super::emit_postcondition_and_close`) keep compiling
// without churn.
pub(super) use super::verify_script_close::{
    emit_postcondition_and_close, emit_verify_step, InterfaceCtx, PostconditionCtx,
};

/// Extract `N` from `llvm_array N (llvm_int 8)`.
fn parse_array_len(saw_type: &str) -> Option<usize> {
    let s = saw_type.strip_prefix("llvm_array ")?;
    let end = s.find(|c: char| !c.is_ascii_digit())?;
    s[..end].parse().ok()
}

#[cfg(test)]
#[path = "verify_script_steps_tests.rs"]
mod tests;
