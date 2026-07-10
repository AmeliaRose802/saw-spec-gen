//! Tests for [`super`] — extracted to keep the parent under the
//! 500 NWS-line limit.

use super::*;
use crate::transform::extern_override_scan::{BrokenReason, OverrideTarget};

pub(super) fn target(
    symbol: &str,
    params: &[&str],
    ret: &str,
    variadic: bool,
    reason: BrokenReason,
) -> OverrideTarget {
    target_with_writes(symbol, params, ret, variadic, reason, &[])
}

/// Same as [`target`] but lets the test specify which mutable globals
/// the scanner would have observed this target's body writing. Used
/// by `globals_get_adversarial_post_clobber` to assert that the
/// emitter clobbers exactly the per-target written set rather than
/// the module-wide mutable-global list.
pub(super) fn target_with_writes(
    symbol: &str,
    params: &[&str],
    ret: &str,
    variadic: bool,
    reason: BrokenReason,
    globals_written: &[&str],
) -> OverrideTarget {
    OverrideTarget {
        symbol: symbol.to_string(),
        fixed_param_ir_types: params.iter().map(|s| s.to_string()).collect(),
        return_ir_type: ret.to_string(),
        is_variadic: variadic,
        reason,
        globals_written: globals_written.iter().map(|s| s.to_string()).collect(),
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
    let out = emit_overrides(&[t], &[], &[], &Default::default());

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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
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
    let out = emit_overrides(&[t], &["printf".to_string()], &[], &Default::default());
    assert!(out.is_empty(), "AST-derived spec takes precedence");
    assert!(!out.snippet.contains("ov_printf"));
}

#[test]
fn empty_input_produces_empty_output() {
    let out = emit_overrides(&[], &[], &[], &Default::default());
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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
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
    let out = emit_overrides(&[t], &[], &[], &Default::default());

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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
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
    // Override of a variadic body that the scanner observed storing
    // to user globals (`g_counter`, `g_flag`) must clobber those
    // globals in its post-state. Without this, a verification target
    // that writes a global then calls the extern then reads the
    // global back would falsely VERIFY. See e2e regression at
    // tests/e2e/cases/08-overrides/variadic_global_clobber/.
    let t = target_with_writes(
        "?log_inc@@YAXPEBDZZ",
        &["ptr"],
        "void",
        true,
        BrokenReason::UsesVarargsIntrinsic,
        &["?g_counter@@3IA", "?g_flag@@3_NA"],
    );
    let globals = vec![
        GlobalVarInfo {
            name: "g_counter".to_string(),
            mangled_name: "?g_counter@@3IA".to_string(),
            ty: TypeInfo::UnsignedInt(32),
            init_value: Some("0".to_string()),
            has_static_initializer: false,
        },
        GlobalVarInfo {
            name: "g_flag".to_string(),
            mangled_name: "?g_flag@@3_NA".to_string(),
            ty: TypeInfo::Bool,
            init_value: None,
            has_static_initializer: false,
        },
    ];
    let out = emit_overrides(&[t], &[], &globals, &Default::default());

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
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        !out.snippet.contains("llvm_global"),
        "empty globals slice must emit zero `llvm_global` references; snippet was:\n{}",
        out.snippet
    );
}

#[test]
fn target_writes_no_globals_means_no_global_clobber() {
    // Even when the module DOES have a mutable global declared, an
    // override target whose body the scanner observed touching nothing
    // must NOT clobber it. This is the printf-doesn't-write-user-globals
    // case: on Windows MSVC, `printf` has a body that uses
    // `llvm.va_start`/`llvm.va_end` and so lands in the override set,
    // but its body chain (`_vfprintf_l` → `__stdio_common_vfprintf`)
    // never `store`s through any user global. Conservatively havocing
    // every module-wide mutable global here produced false DISPROVED
    // verdicts for tests like
    // `tests/e2e/cases/02-havoc-coverage/concrete_type_safe/
    // add_one_verified.cpp` whose `add_one` reads `super_important`
    // after calling printf.
    use crate::constraints::{GlobalVarInfo, TypeInfo};
    let t = target_with_writes(
        "printf",
        &["ptr"],
        "i32",
        true,
        BrokenReason::UsesVarargsIntrinsic,
        &[], // scanner saw printf write nothing
    );
    let globals = vec![GlobalVarInfo {
        name: "super_important".to_string(),
        mangled_name: "?super_important@@3HA".to_string(),
        ty: TypeInfo::SignedInt(32),
        init_value: Some("7".to_string()),
        has_static_initializer: false,
    }];
    let out = emit_overrides(&[t], &[], &globals, &Default::default());
    assert!(
        !out.snippet.contains("super_important"),
        "printf-style override (empty globals_written) must not clobber \
         any module-wide global; snippet was:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("llvm_global"),
        "no llvm_global reference should appear when the target writes \
         nothing; snippet was:\n{}",
        out.snippet
    );
}

#[test]
fn declare_only_extern_clobbers_globals_in_written_set() {
    // DeclareOnly targets carry `globals_written` populated by the
    // scanner (all externally-visible mutable globals, conservatively).
    // The emitter must clobber exactly those globals. This pairs with
    // `target_writes_no_globals_means_no_global_clobber` to confirm
    // the emitter faithfully follows the scanner's written set.
    use crate::constraints::{GlobalVarInfo, TypeInfo};
    let t = target_with_writes(
        "__stdio_common_vfprintf",
        &["i64", "ptr", "ptr", "ptr", "ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
        &["?user_state@@3HA"], // scanner fills this for declare-only
    );
    let globals = vec![GlobalVarInfo {
        name: "user_state".to_string(),
        mangled_name: "?user_state@@3HA".to_string(),
        ty: TypeInfo::SignedInt(32),
        init_value: None,
        has_static_initializer: false,
    }];
    let out = emit_overrides(&[t], &[], &globals, &Default::default());
    assert!(
        out.snippet.contains("user_state"),
        "declare-only extern with globals_written must havoc those globals; \
         snippet was:\n{}",
        out.snippet
    );
}

#[test]
fn mutex_lock_pins_success_sentinel_return() {
    // `_Mtx_lock` returns `_Thrd_result` (i32) whose success value
    // `_Thrd_success` is 0. A lock-guarded body (`std::scoped_lock`)
    // pulls it in on every path; a fresh symbolic return lets the
    // solver pick a failure code and sends `_Mutex_base::lock` down its
    // `_Throw_Cpp_error` → `unreachable` path, failing the subgoal. The
    // override must instead pin the success sentinel. See doc item 5 in
    // docs/03-stateful-method-specs.md.
    let t = target(
        "_Mtx_lock",
        &["ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        out.snippet
            .contains("llvm_return (llvm_term {{ 0 : [32] }});"),
        "mutex lock override must pin the success sentinel; snippet was:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("rv <- llvm_fresh_var"),
        "success-sentinel return must NOT allocate a fresh symbolic rv; \
         snippet was:\n{}",
        out.snippet
    );
    // A human-readable marker so the pinned value isn't mysterious.
    assert!(out.snippet.contains("_Thrd_success"));
}

#[test]
fn mutex_unlock_and_init_destroy_pin_success_sentinel() {
    for sym in ["_Mtx_unlock", "_Mtx_init", "_Mtx_destroy"] {
        let t = target(sym, &["ptr"], "i32", false, BrokenReason::DeclareOnly);
        let out = emit_overrides(&[t], &[], &[], &Default::default());
        assert!(
            out.snippet
                .contains("llvm_return (llvm_term {{ 0 : [32] }});"),
            "{sym} must pin the success sentinel; snippet was:\n{}",
            out.snippet
        );
        assert!(
            !out.snippet.contains("rv <- llvm_fresh_var"),
            "{sym} must not emit a fresh symbolic return; snippet was:\n{}",
            out.snippet
        );
    }
}

#[test]
fn mutex_trylock_is_not_pinned() {
    // `_Mtx_trylock` has a legitimate `_Thrd_busy` return; pinning
    // success would silently erase the busy path, so it is deliberately
    // excluded from the sentinel set and keeps the default fresh return.
    let t = target(
        "_Mtx_trylock",
        &["ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        out.snippet
            .contains("rv <- llvm_fresh_var \"rv\" (llvm_int 32);"),
        "trylock must keep a fresh symbolic return; snippet was:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("_Thrd_success"),
        "trylock must not be pinned to the success sentinel"
    );
}

#[test]
fn ordinary_int_return_is_not_pinned() {
    // A symbol that is not a curated status primitive must keep the
    // default fresh symbolic return — the sentinel rule is exact-match,
    // never a substring or family heuristic.
    let t = target(
        "compute_checksum",
        &["ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        out.snippet
            .contains("rv <- llvm_fresh_var \"rv\" (llvm_int 32);"),
        "non-primitive int return must stay symbolic; snippet was:\n{}",
        out.snippet
    );
    assert!(!out.snippet.contains("_Thrd_success"));
}

#[path = "bitcode_overrides_tests_msvc.rs"]
mod msvc_tests;
