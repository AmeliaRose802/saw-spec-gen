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
    let w = super::super::cryptol_sig_parse::cryptol_param_widths_from_str(cry, "f").unwrap();
    assert_eq!(w, vec![8]);
}

#[test]
fn cryptol_widths_multi() {
    let cry = "f : Bit -> [8] -> [32] -> [16]\n";
    let w = super::super::cryptol_sig_parse::cryptol_param_widths_from_str(cry, "f").unwrap();
    assert_eq!(w, vec![1, 8, 32]);
}

#[test]
fn cryptol_widths_byte_array() {
    let cry = "g : [4][8] -> [32]\n";
    let w = super::super::cryptol_sig_parse::cryptol_param_widths_from_str(cry, "g").unwrap();
    assert_eq!(w, vec![32]);
}

// ─── Aggregate bridge tests ────────────────────────────────────────

#[test]
fn pack_int_two_fields_little_endian() {
    let bridge = AbiReturnBridge::PackInt {
        fields: vec![
            PackedField {
                offset_bits: 0,
                width: 8,
            },
            PackedField {
                offset_bits: 8,
                width: 8,
            },
        ],
        total_bits: 16,
        endian: Endianness::Little,
    };
    let result = bridge.wrap("f x");
    assert!(result.contains("zext`{16}"), "missing zext: {result}");
    assert!(result.contains("<< 8"), "missing shift: {result}");
    assert!(result.contains("||"), "missing OR: {result}");
}

#[test]
fn struct_value_two_bools() {
    let bridge = AbiReturnBridge::StructValue {
        field_bits: vec![1, 1],
    };
    let saw = bridge.emit_saw_return("f x");
    assert!(
        saw.contains("llvm_struct_value"),
        "missing struct_value: {saw}"
    );
    assert!(saw.contains("(f x).0"), "missing field .0: {saw}");
    assert!(saw.contains("(f x).1"), "missing field .1: {saw}");
    assert!(saw.contains("[1]"), "missing Bit→[1] bridge: {saw}");
}

#[test]
fn struct_bytes_no_preserved() {
    let bridge = AbiReturnBridge::StructBytes {
        fields: vec![ByteField {
            byte_offset: 0,
            width: 32,
        }],
        total_bytes: 4,
        preserved: vec![],
    };
    let saw = bridge.emit_saw_return("f x");
    assert!(
        saw.contains("llvm_points_to result_ptr"),
        "missing points_to: {saw}"
    );
}

#[test]
fn struct_bytes_with_preserved() {
    let bridge = AbiReturnBridge::StructBytes {
        fields: vec![ByteField {
            byte_offset: 0,
            width: 8,
        }],
        total_bytes: 4,
        preserved: vec![PreservedRange { start: 1, len: 3 }],
    };
    let saw = bridge.emit_saw_return("f x");
    assert!(
        saw.contains("preserved from prestate"),
        "missing preserved comment: {saw}"
    );
}

#[test]
fn variant_remap_two_variants() {
    let bridge = AbiReturnBridge::VariantRemap {
        variants: vec![(0, 0), (1, 1)],
        abi_bits: 8,
        inner: None,
    };
    let result = bridge.wrap("f x");
    assert!(result.contains("if"), "missing if: {result}");
    assert!(result.contains("then (0 : [8])"), "missing then: {result}");
    assert!(result.contains("else (1 : [8])"), "missing else: {result}");
}

#[test]
fn variant_remap_with_width_bridge() {
    let bridge = AbiReturnBridge::VariantRemap {
        variants: vec![(0, 0), (1, 1)],
        abi_bits: 1,
        inner: Some(Box::new(AbiReturnBridge::Truncate {
            cry_bits: 8,
            llvm_bits: 1,
        })),
    };
    let result = bridge.wrap("f x");
    assert!(result.contains("drop`{7}"), "missing truncate: {result}");
}

#[test]
fn variant_remap_three_variants() {
    let bridge = AbiReturnBridge::VariantRemap {
        variants: vec![(0, 0), (1, 1), (2, 2)],
        abi_bits: 8,
        inner: None,
    };
    let result = bridge.wrap("f x");
    // Should generate nested if-then-else
    assert!(
        result.matches("if").count() >= 2,
        "expected nested ifs: {result}"
    );
}
