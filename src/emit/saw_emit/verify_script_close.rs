//! Postcondition + verification-step emitters extracted from
//! [`super::verify_script_steps`] to stay under the 500-NWS limit.

use super::cryptol_bridge::{cryptol_return_for, AbiReturnBridge};
use super::names::{sanitize_name, stub_function_name};
use crate::buffer_overrides::BufferOverrides;
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::{SpecConstraint, TypeInfo};
use std::collections::HashSet;

/// Groups the return-type / sret / callee context needed by the
/// postcondition emitter, keeping the arg count under clippy's limit.
pub(super) struct PostconditionCtx<'a> {
    pub sub_callee_specs: &'a [SpecConstraint],
    pub return_type: &'a TypeInfo,
    pub is_sret: bool,
    /// Optional aggregate return bridge. When set, overrides the
    /// default scalar bridge for the return assertion.
    pub return_bridge: Option<&'a AbiReturnBridge>,
    /// Auto-detected output-buffer postconditions: pairs of
    /// `(param_name, cryptol_fn_name)` populated by the
    /// `apply_out_postcond_autodetect` pass.  Each entry emits
    /// `llvm_points_to <name>_ptr (llvm_term {{ <fn> args }})` after
    /// `llvm_execute_func`.
    pub auto_out_postconds: &'a [(String, String)],
}

pub(super) fn emit_postcondition_and_close(
    out: &mut String,
    cryptol_fn: &str,
    cryptol_args: &[String],
    execute_args: &[String],
    ctx: &PostconditionCtx<'_>,
    buffer_overrides: &BufferOverrides,
) {
    out.push_str(&format!(
        "    llvm_execute_func [{}];\n\n",
        execute_args.join(", "),
    ));

    // Output-buffer postconditions (--out-buffer-param + --cryptol-fn-out):
    // assert each writable buffer holds the bytes computed by the
    // associated Cryptol function. Emitted BEFORE the return-value
    // assertion so the script reads top-to-bottom in execution order.
    // Default arg list = the full positional C-param list (with
    // out-buffer params already rewritten to `<name>_pre`), so a
    // Cryptol fn that mirrors the C signature 1:1 needs no
    // `--cryptol-arg-order` override. Set the flag only when the
    // Cryptol model takes a subset / different order.
    for (out_name, fn_name) in &buffer_overrides.cryptol_fn_out {
        let args = buffer_overrides
            .cryptol_call_args(fn_name)
            .unwrap_or_else(|| cryptol_args.to_vec());
        let call = if args.is_empty() {
            fn_name.clone()
        } else {
            format!("{} {}", fn_name, args.join(" "))
        };
        out.push_str(&format!(
            "    // Postcondition: *{out_name}_ptr == Cryptol {fn_name}\n",
        ));
        out.push_str(&format!(
            "    llvm_points_to {out_name}_ptr (llvm_term {{{{ {call} }}}});\n",
        ));
    }

    // Auto-detected output-buffer postconditions from _Out_writes_ + <param>_post convention.
    for (out_name, fn_name) in ctx.auto_out_postconds {
        let pre_name = format!("{out_name}_pre");
        let args: Vec<String> = cryptol_args
            .iter()
            .map(|a| {
                if a == out_name {
                    pre_name.clone()
                } else {
                    a.clone()
                }
            })
            .collect();
        let call = if args.is_empty() {
            fn_name.clone()
        } else {
            format!("{} {}", fn_name, args.join(" "))
        };
        out.push_str(&format!(
            "    // Postcondition: *{out_name}_ptr == Cryptol {fn_name} (auto from _Out_writes_)\n",
        ));
        out.push_str(&format!(
            "    llvm_points_to {out_name}_ptr (llvm_term {{{{ {call} }}}});\n",
        ));
    }

    // Honor --cryptol-arg-order for the *return-value* Cryptol fn.
    // When set, override the auto-derived positional arg list.
    let cryptol_args_owned: Vec<String> = buffer_overrides
        .cryptol_call_args(cryptol_fn)
        .unwrap_or_else(|| cryptol_args.to_vec());
    let cryptol_call = if cryptol_args_owned.is_empty() {
        cryptol_fn.to_string()
    } else {
        format!("{} {}", cryptol_fn, cryptol_args_owned.join(" "))
    };
    let cryptol_return = cryptol_return_for(&cryptol_call, ctx.return_type);
    let is_void_return = matches!(ctx.return_type, TypeInfo::Void);

    // If an aggregate return bridge was supplied, use it instead of
    // the default scalar bridge.
    if let Some(bridge) = ctx.return_bridge {
        out.push_str(&bridge.emit_saw_return(&cryptol_call));
        out.push_str("};\n\n");
        return;
    }

    if ctx.is_sret {
        if ctx.sub_callee_specs.is_empty() {
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
        out.push_str("    // Void return — no llvm_return to emit.\n");
        out.push_str(&format!(
            "    // (Cryptol spec `{}` is referenced for documentation only.)\n",
            cryptol_return,
        ));
    } else if ctx.sub_callee_specs.is_empty() {
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

/// Groups the C++ interface / vtable context needed by the verify-step
/// emitter, keeping the arg count under clippy's limit.
pub(super) struct InterfaceCtx<'a> {
    pub has_interfaces: bool,
    pub vmethods: &'a [InterfaceMethod],
    pub constructors: &'a [ClassConstructor],
}

pub(super) fn emit_verify_step(
    out: &mut String,
    step: u32,
    function_name: &str,
    cryptol_fn: &str,
    mangled_name: &str,
    iface: &InterfaceCtx<'_>,
    override_names: Vec<String>,
) {
    out.push_str(&format!("// Step {step}: Verify equivalence\n"));
    // Machine-readable proof marker (see docs/proof-markers.md). Must
    // precede every verification command so downstream aggregators can
    // attribute counterexamples / warnings to a specific property.
    out.push_str(&format!("print \"BEGIN_PROOF {function_name}\";\n"));
    out.push_str(&format!(
        "print \"=== Checking: {function_name} == {cryptol_fn} (Cryptol) ===\";\n",
    ));

    let mut overrides = override_names;
    if iface.has_interfaces {
        let originating_names: HashSet<&str> = iface
            .vmethods
            .iter()
            .filter(|m| !m.is_override)
            .map(|m| m.method.name.as_str())
            .collect();
        let ctor_classes: HashSet<String> = iface
            .constructors
            .iter()
            // Itanium ctor overrides are intentionally not emitted.
            .filter(|c| !c.mangled_name.starts_with("_Z"))
            .map(|c| sanitize_name(&c.class_name).to_lowercase())
            .collect();
        for safe_class in &ctor_classes {
            overrides.push(format!("ov_{safe_class}_ctor"));
        }
        for method in iface.vmethods {
            if method.is_override && originating_names.contains(method.method.name.as_str()) {
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
    // Closing marker — emitted only on success, since SAW aborts the
    // script on a failed `llvm_verify`. Absence of this line after a
    // BEGIN_PROOF is the signal of failure.
    out.push_str(&format!("print \"PROVED {function_name}\";\n"));
}
