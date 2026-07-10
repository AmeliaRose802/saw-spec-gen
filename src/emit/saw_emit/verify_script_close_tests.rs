//! Tests for loop-invariant / fixpoint-CHC mode in [`super::verify_script_close`].

use super::*;

#[test]
fn emit_verify_step_uses_fixpoint_chc_with_loop_invariants() {
    let mut out = String::new();
    let iface = InterfaceCtx {
        has_interfaces: false,
        vmethods: &[],
        constructors: &[],
    };
    let invariants = vec!["scan_inv".to_string()];
    emit_verify_step(
        &mut out,
        1,
        "f",
        "f_spec",
        "_Zf",
        &iface,
        vec![],
        &invariants,
    );
    assert!(out.contains("llvm_verify_fixpoint_chc"), "got:\n{out}");
    assert!(out.contains("// proof_mode: invariant"), "got:\n{out}");
    assert!(out.contains("//   - scan_inv"), "got:\n{out}");
    assert!(!out.contains("llvm_verify m "), "got:\n{out}");
}
