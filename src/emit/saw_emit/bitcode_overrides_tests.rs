//! Tests for [`super`] — extracted to keep the parent under the
//! 500 NWS-line limit.

use super::*;
use crate::transform::extern_override_scan::{BrokenReason, OverrideTarget};

fn target(
    symbol: &str,
    params: &[&str],
    ret: &str,
    variadic: bool,
    reason: BrokenReason,
) -> OverrideTarget {
    OverrideTarget {
        symbol: symbol.to_string(),
        fixed_param_ir_types: params.iter().map(|s| s.to_string()).collect(),
        return_ir_type: ret.to_string(),
        is_variadic: variadic,
        reason,
    }
}

#[test]
fn variadic_printf_uses_only_fixed_args_in_execute_func() {
    // The probe in tests/e2e/cases/08-overrides/bump/.../verify_probe3.saw
    // demonstrated that SAW's matcher accepts variadic call sites against
    // a fixed-prefix spec. If the emitter ever regressed to including the
    // vararg portion, every bump-style test would start failing with
    // "Fresh variable(s) not reachable via points-tos" — this test locks
    // that contract in place.
    let t = target(
        "printf",
        &["ptr"],
        "i32",
        true,
        BrokenReason::UsesVarargsIntrinsic,
    );
    let out = emit_overrides(&[t], &[], &[]);

    assert!(out
        .snippet
        .contains("ov_printf <- llvm_unsafe_assume_spec m \"printf\""));
    assert!(out.snippet.contains("llvm_execute_func [p0];"));
    assert!(out
        .snippet
        .contains("p0 <- llvm_fresh_pointer (llvm_int 8);"));
    // Variadic-aware comment so a human reader knows why this exists.
    assert!(out.snippet.contains("[body uses llvm.va_*; variadic]"));
    assert_eq!(out.override_names, vec!["ov_printf".to_string()]);
}

#[test]
fn declare_only_ucrt_extern_gets_full_signature() {
    let t = target(
        "__stdio_common_vfprintf",
        &["i64", "ptr", "ptr", "ptr", "ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[]);
    assert!(out
        .snippet
        .contains("p0 <- llvm_fresh_var \"p0\" (llvm_int 64);"));
    assert!(out
        .snippet
        .contains("p1 <- llvm_fresh_pointer (llvm_int 8);"));
    assert!(out
        .snippet
        .contains("llvm_execute_func [llvm_term p0, p1, p2, p3, p4];"));
    assert!(out
        .snippet
        .contains("rv <- llvm_fresh_var \"rv\" (llvm_int 32);"));
    assert!(out.snippet.contains("llvm_return (llvm_term rv);"));
    assert!(out.snippet.contains("[declare-only]"));
}

#[test]
fn void_return_omits_llvm_return() {
    let t = target("free", &["ptr"], "void", false, BrokenReason::DeclareOnly);
    let out = emit_overrides(&[t], &[], &[]);
    assert!(out.snippet.contains("llvm_execute_func [p0];"));
    assert!(
        !out.snippet.contains("llvm_return"),
        "void-returning override must not emit llvm_return"
    );
}

#[test]
fn pointer_return_uses_fresh_pointer_not_fresh_var() {
    let t = target(
        "__acrt_iob_func",
        &["i32"],
        "ptr",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[]);
    assert!(out
        .snippet
        .contains("rv <- llvm_fresh_pointer (llvm_int 8);"));
    assert!(out.snippet.contains("llvm_return rv;"));
    assert!(
        !out.snippet.contains("llvm_term rv"),
        "ptr returns wrap the value directly, not via llvm_term"
    );
}

#[test]
fn already_covered_symbols_are_skipped() {
    let t = target("printf", &["ptr"], "i32", true, BrokenReason::DeclareOnly);
    // Caller already emits an AST-derived spec for printf.
    let out = emit_overrides(&[t], &["printf".to_string()], &[]);
    assert!(out.is_empty(), "AST-derived spec takes precedence");
    assert!(!out.snippet.contains("ov_printf"));
}

#[test]
fn empty_input_produces_empty_output() {
    let out = emit_overrides(&[], &[], &[]);
    assert!(out.is_empty());
    assert!(out.snippet.is_empty());
}

#[test]
fn mangled_symbols_sanitize_safely() {
    // `?` and `@` in MSVC mangled names must be sanitized into valid
    // SAWScript identifiers, but the literal symbol must still appear
    // inside the `llvm_unsafe_assume_spec` quoted string.
    let t = target(
        "?bar@@YAHXZ",
        &["i32"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[]);
    assert!(
        out.snippet.contains("\"?bar@@YAHXZ\""),
        "raw mangled name preserved inside the assume_spec target string"
    );
    // The override variable name itself must be a legal SAW identifier.
    let name = &out.override_names[0];
    assert!(name.starts_with("ov_"));
    assert!(!name.contains('?'));
    assert!(!name.contains('@'));
}

#[test]
fn pointer_params_get_adversarial_post_clobber() {
    // Regression test for the variadic_clobber gap. Before this contract
    // landed, an override with a ptr parameter emitted no post-condition
    // and SAW treated pointee memory as preserved across the call,
    // silently producing false positives for any callee that wrote
    // through its pointer arg (e.g. a variadic logger that captured a
    // value into an out-param). See
    // `tests/e2e/cases/08-overrides/variadic_clobber/add_one_disproved.cpp`
    // for the empirical case that pre-fix VERIFIED and post-fix DISPROVES.
    //
    // The override must emit, per ptr param, an `llvm_points_to_at_type`
    // post-condition writing one fresh symbolic byte (one byte is the
    // widest universally-safe clobber — see the module docstring).
    let t = target(
        "log_and_capture",
        &["ptr", "ptr"],
        "void",
        true,
        BrokenReason::UsesVarargsIntrinsic,
    );
    let out = emit_overrides(&[t], &[], &[]);

    // Both ptr params have post-clobber pairs.
    assert!(
        out.snippet
            .contains("p0_after <- llvm_fresh_var \"p0_after\" (llvm_int 8);"),
        "missing fresh symbolic byte for p0; snippet was:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains("llvm_points_to_at_type p0 (llvm_int 8) (llvm_term p0_after);"),
        "missing post-clobber for p0; snippet was:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains("p1_after <- llvm_fresh_var \"p1_after\" (llvm_int 8);"),
        "missing fresh symbolic byte for p1"
    );
    assert!(
        out.snippet
            .contains("llvm_points_to_at_type p1 (llvm_int 8) (llvm_term p1_after);"),
        "missing post-clobber for p1"
    );

    // The clobber must appear AFTER `llvm_execute_func`, not before.
    let exec_idx = out
        .snippet
        .find("llvm_execute_func")
        .expect("execute_func must be present");
    let clobber_idx = out
        .snippet
        .find("llvm_points_to_at_type")
        .expect("clobber must be present");
    assert!(
        clobber_idx > exec_idx,
        "post-condition clobber must appear after llvm_execute_func"
    );
}

#[test]
fn int_only_params_get_no_clobber() {
    // Overrides whose fixed prefix has zero pointer params must NOT
    // emit `llvm_points_to_at_type` — there is nothing to clobber, and
    // emitting it would be a SAWScript error (no ptr binding in scope).
    let t = target(
        "compute_hash",
        &["i64", "i32"],
        "i64",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[]);
    assert!(
        !out.snippet.contains("llvm_points_to_at_type"),
        "non-ptr override must not emit any pointee clobber; snippet was:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("_after"),
        "non-ptr override must not allocate `_after` fresh vars"
    );
}

#[test]
fn globals_get_adversarial_post_clobber() {
    use crate::constraints::{GlobalVarInfo, TypeInfo};
    // Override of any extern must clobber every mutable global the
    // caller passed in — the extern is unknown, so we must conservatively
    // assume it touched all of them. Without this, a verification target
    // that writes a global then calls the extern then reads the global
    // would falsely VERIFY. See e2e regression at
    // tests/e2e/cases/08-overrides/variadic_global_clobber/.
    let t = target(
        "?log_inc@@YAXPEBDZZ",
        &["ptr"],
        "void",
        true,
        BrokenReason::UsesVarargsIntrinsic,
    );
    let globals = vec![
        GlobalVarInfo {
            name: "g_counter".to_string(),
            mangled_name: "?g_counter@@3IA".to_string(),
            ty: TypeInfo::UnsignedInt(32),
            init_value: Some("0".to_string()),
        },
        GlobalVarInfo {
            name: "g_flag".to_string(),
            mangled_name: "?g_flag@@3_NA".to_string(),
            ty: TypeInfo::Bool,
            init_value: None,
        },
    ];
    let out = emit_overrides(&[t], &[], &globals);

    // Fresh + points-to pair for each global.
    assert!(
        out.snippet
            .contains("g_counter_after <- llvm_fresh_var \"g_counter_after\" (llvm_int 32);"),
        "missing fresh var for g_counter; snippet was:\n{}",
        out.snippet
    );
    assert!(
        out.snippet.contains(
            "llvm_points_to (llvm_global \"?g_counter@@3IA\") (llvm_term g_counter_after);"
        ),
        "missing points-to for g_counter; snippet was:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains("g_flag_after <- llvm_fresh_var \"g_flag_after\" (llvm_int 1);"),
        "missing fresh var for g_flag (Bool width=1); snippet was:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains("llvm_points_to (llvm_global \"?g_flag@@3_NA\") (llvm_term g_flag_after);"),
        "missing points-to for g_flag; snippet was:\n{}",
        out.snippet
    );

    // Clobber must appear AFTER execute_func, before the closing `};`.
    let exec_idx = out.snippet.find("llvm_execute_func").unwrap();
    let glob_idx = out.snippet.find("g_counter_after").unwrap();
    assert!(
        glob_idx > exec_idx,
        "global clobber must follow llvm_execute_func"
    );
}

#[test]
fn no_globals_means_no_global_clobber_emitted() {
    // Empty mutable_globals slice must produce zero global writes —
    // otherwise we'd emit a stray `llvm_points_to (llvm_global ...)` for
    // a symbol the equiv spec never allocated, which SAW rejects.
    let t = target(
        "?log_inc@@YAXPEBDZZ",
        &["ptr"],
        "void",
        true,
        BrokenReason::UsesVarargsIntrinsic,
    );
    let out = emit_overrides(&[t], &[], &[]);
    assert!(
        !out.snippet.contains("llvm_global"),
        "empty globals slice must emit zero `llvm_global` references; snippet was:\n{}",
        out.snippet
    );
}
