//! Tests for the ABI-bridge helpers in `cryptol_bridge.rs`.

use super::*;

#[test]
fn param_bridge_identity() {
    assert_eq!(param_bridge(32, 32), AbiParamBridge::Identity);
}

#[test]
fn param_bridge_bit() {
    let b = param_bridge(1, 1);
    assert_eq!(b, AbiParamBridge::BitExtract);
    assert_eq!(b.wrap("x"), "(x ! 0)");
}

#[test]
fn param_bridge_trunc() {
    let b = param_bridge(8, 3);
    assert_eq!(b.wrap("x"), "drop`{5} x");
}

#[test]
fn return_bridge_identity() {
    assert_eq!(return_bridge(32, 32), AbiReturnBridge::Identity);
}

#[test]
fn return_bridge_bit() {
    let b = return_bridge(1, 1);
    assert_eq!(b.wrap("f x"), "[f x] : [1]");
}

#[test]
fn return_bridge_zext() {
    let b = return_bridge(3, 8);
    assert_eq!(b.wrap("f x"), "zext`{8} (f x)");
}

#[test]
fn cryptol_widths_simple() {
    let cry = "f : [8] -> [32]\n";
    let w = cryptol_param_widths_from_str(cry, "f").unwrap();
    assert_eq!(w, vec![8]);
}

#[test]
fn cryptol_widths_multi() {
    let cry = "f : Bit -> [8] -> [32] -> [16]\n";
    let w = cryptol_param_widths_from_str(cry, "f").unwrap();
    assert_eq!(w, vec![1, 8, 32]);
}

#[test]
fn cryptol_widths_byte_array() {
    let cry = "g : [4][8] -> [32]\n";
    let w = cryptol_param_widths_from_str(cry, "g").unwrap();
    assert_eq!(w, vec![32]);
}
