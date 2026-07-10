//! Status-primitive / mutex success-sentinel tests for [`super`] --
//! split out of `bitcode_overrides_tests.rs` to keep both files under
//! the 500 NWS-line limit. Covers the MSVC `_Mtx_*` and libstdc++
//! `pthread_mutex_*` sentinel pins plus the MSVC `_Mutex_base` helper
//! no-op, i.e. the cross-platform mutex override contract exercised by
//! tests/e2e/cases/08-overrides/{mutex_sentinel,msvc_mutex_helper}.

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
        globals_written: Vec::new(),
    }
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
fn posix_pthread_mutex_pins_success_sentinel() {
    // libstdc++/glibc lock guards lower to the POSIX `pthread_mutex_*`
    // primitives (whose success return is 0), either directly or via a
    // `__gthrw_`-prefixed weakref alias. Both forms must pin the success
    // sentinel so the Linux mutex e2e verifies through the same
    // sequential-lock argument as the MSVC `_Mtx_*` path does on Windows.
    for sym in [
        "pthread_mutex_lock",
        "pthread_mutex_unlock",
        "pthread_mutex_init",
        "pthread_mutex_destroy",
        "__gthrw_pthread_mutex_lock",
        "__gthrw_pthread_mutex_unlock",
    ] {
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
fn posix_pthread_mutex_trylock_is_not_pinned() {
    // `pthread_mutex_trylock` has a legitimate `EBUSY` return; pinning
    // success would erase the busy path, so it keeps a fresh return.
    let t = target(
        "pthread_mutex_trylock",
        &["ptr"],
        "i32",
        false,
        BrokenReason::DeclareOnly,
    );
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        out.snippet
            .contains("rv <- llvm_fresh_var \"rv\" (llvm_int 32);"),
        "pthread_mutex_trylock must keep a fresh symbolic return; snippet was:\n{}",
        out.snippet
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

#[test]
fn msvc_mutex_helper_pins_noop_bool_return() {
    let sym = "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ";
    let t = target(sym, &["ptr"], "i1", false, BrokenReason::MsvcMutexHelper);
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    let snip = &out.snippet;
    assert!(snip.contains("{{ 1 : [1] }}") && snip.contains("[msvc-mutex-helper]"));
}
