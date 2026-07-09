//! Mutex sentinel and MSVC mutex-helper override tests — extracted from
//! `bitcode_overrides_tests` to keep the parent under the 500 NWS line
//! limit.

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
        globals_written: vec![],
    }
}

#[test]
fn mutex_lock_pins_success_sentinel_return() {
    // `_Mtx_lock` returns `_Thrd_result` (i32); success value is 0.
    // A fresh symbolic return lets the solver pick a failure code and
    // sends `_Mutex_base::lock` down `_Throw_Cpp_error` → `unreachable`.
    // The override must pin the sentinel instead.
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
        "mutex lock must pin the success sentinel; snippet:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("rv <- llvm_fresh_var"),
        "sentinel return must not allocate a fresh rv; snippet:\n{}",
        out.snippet
    );
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
            "{sym} must pin the success sentinel; snippet:\n{}",
            out.snippet
        );
        assert!(
            !out.snippet.contains("rv <- llvm_fresh_var"),
            "{sym} must not emit a fresh symbolic return; snippet:\n{}",
            out.snippet
        );
    }
}

#[test]
fn mutex_trylock_is_not_pinned() {
    // `_Mtx_trylock` has a legitimate `_Thrd_busy` return; pinning
    // success would silently erase the busy path.
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
        "trylock must keep a fresh symbolic return; snippet:\n{}",
        out.snippet
    );
    assert!(
        !out.snippet.contains("_Thrd_success"),
        "trylock must not be pinned to the success sentinel"
    );
}

#[test]
fn ordinary_int_return_is_not_pinned() {
    // A non-curated symbol must keep the default fresh symbolic return.
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
        "non-primitive int return must stay symbolic; snippet:\n{}",
        out.snippet
    );
    assert!(!out.snippet.contains("_Thrd_success"));
}

#[test]
fn msvc_mutex_helper_gets_override_and_fresh_return() {
    // Issue #65: `_Verify_ownership_levels@_Mutex_base@std` is defined
    // linkonce_odr but causes "Error during memory load" in SAW.
    // Emitter must produce an llvm_unsafe_assume_spec tagged
    // "msvc-mutex-helper" with a fresh i1 return (not in the
    // success-sentinel set; caller discards the value).
    let sym = "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ";
    let t = target(sym, &["ptr"], "i1", false, BrokenReason::MsvcMutexHelper);
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(
        out.snippet.contains("[msvc-mutex-helper]"),
        "must tag the comment; snippet:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains(&format!("llvm_unsafe_assume_spec m \"{sym}\"")),
        "must reference the mangled symbol; snippet:\n{}",
        out.snippet
    );
    assert!(
        out.snippet
            .contains("rv <- llvm_fresh_var \"rv\" (llvm_int 1);"),
        "i1 return must be fresh; snippet:\n{}",
        out.snippet
    );
    assert!(out.snippet.contains("llvm_return (llvm_term rv);"));
    assert_eq!(out.override_names.len(), 1);
}
