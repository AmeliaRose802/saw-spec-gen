//! MSVC-specific override emission tests extracted from the parent test module
//! to keep `bitcode_overrides_tests.rs` under the 500 NWS-line limit.

use super::super::*;
use crate::emit::saw_emit::bitcode_overrides_functional::StlMethod;
use crate::emit::saw_emit::bitcode_overrides_functional_string::{
    emit_string_override, StringLayout,
};
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

#[test]
fn msvc_resize_emits_char_param() {
    // MSVC resize(size_t, char) — third param is the defaulted char.
    let layout = StringLayout {
        alias: "class.std::basic_string".to_string(),
        size_field_index: 1,
    };
    let sym = "?resize@?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAAX_KD@Z";
    let mut out = String::new();
    emit_string_override(
        &mut out,
        StlMethod::BasicStringResize,
        &layout,
        sym,
        "rv",
        "ov_rv",
    );
    assert!(out.contains("llvm_fresh_var \"ch\" (llvm_int 8)"));
    assert!(out.contains("llvm_execute_func [s, llvm_term n, llvm_term ch]"));
}
