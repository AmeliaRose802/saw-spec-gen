//! MSVC-specific override emission tests extracted from the parent test module
//! to keep `bitcode_overrides_tests.rs` under the 500 NWS-line limit.

use super::super::*;
use crate::transform::extern_override_scan::BrokenReason;

#[test]
fn byte_array_return_emits_llvm_array_fresh_var() {
    let t = super::target("f", &[], "[16 x i8]", false, BrokenReason::DeclareOnly);
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    assert!(out
        .snippet
        .contains("llvm_fresh_var \"rv\" (llvm_array 16 (llvm_int 8))"));
}

#[test]
fn msvc_mutex_helper_pins_noop_bool_return() {
    let sym = "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ";
    let t = super::target(sym, &["ptr"], "i1", false, BrokenReason::MsvcMutexHelper);
    let out = emit_overrides(&[t], &[], &[], &Default::default());
    let snip = &out.snippet;
    assert!(snip.contains("{{ 1 : [1] }}") && snip.contains("[msvc-mutex-helper]"));
}
