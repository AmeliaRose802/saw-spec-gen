use super::*;
use crate::llvm_ir::struct_defs;

const MSVC: &str =
    "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128";
const ITANIUM: &str =
    "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128";
const I386: &str = "e-m:e-p:32:32-i64:32:64-f80:32-n8:16:32-S128";

fn check(
    dl: &DataLayout,
    ty: &str,
    defs: &HashMap<String, IrStructDef>,
    size: usize,
    alignment: usize,
    offsets: &[usize],
) {
    assert_eq!(
        dl.layout_of(ty, defs).unwrap(),
        TypeLayout {
            size,
            alignment,
            offsets: offsets.to_vec(),
        },
        "type {ty}"
    );
}

#[test]
fn x86_64_msvc_and_itanium_use_target_not_host_layout() {
    let defs = struct_defs(
        r#"
%"struct.X" = type { i8, i64, ptr, i16 }
%struct.Inner = type { i32, i8 }
%struct.Outer = type { i8, %struct.Inner, i64 }
"#,
    );
    for text in [MSVC, ITANIUM] {
        let dl = DataLayout::parse(text).unwrap();
        assert!(dl.little_endian);
        assert_eq!(dl.pointer_size, 8);
        check(&dl, "%\"struct.X\"", &defs, 32, 8, &[0, 8, 16, 24]);
        check(&dl, "%struct.Inner", &defs, 8, 4, &[0, 4]);
        check(&dl, "%struct.Outer", &defs, 24, 8, &[0, 4, 16]);
        check(&dl, "x86_fp80", &defs, 16, 16, &[]);
    }
}

#[test]
fn i386_pointer_size_and_i64_abi_alignment_are_four_bytes() {
    let dl = DataLayout::parse(I386).unwrap();
    let defs = HashMap::new();
    assert_eq!(dl.pointer_size, 4);
    check(&dl, "ptr", &defs, 4, 4, &[]);
    check(&dl, "{ i8, i64, ptr, i8 }", &defs, 20, 4, &[0, 4, 12, 16]);
    check(&dl, "[2 x i64]", &defs, 16, 4, &[]);
    check(&dl, "x86_fp80", &defs, 12, 4, &[]);
}

#[test]
fn documented_defaults_distinguish_abi_and_preferred_alignment() {
    let dl = DataLayout::parse("e").unwrap();
    let defs = HashMap::new();
    assert_eq!(dl.pointer_size, 8);
    for (ty, size, alignment) in [
        ("i1", 1, 1),
        ("i8", 1, 1),
        ("i16", 2, 2),
        ("i32", 4, 4),
        ("i64", 8, 4),
        ("i128", 16, 4),
        ("ptr", 8, 8),
        ("half", 2, 2),
        ("bfloat", 2, 2),
        ("float", 4, 4),
        ("double", 8, 8),
        ("fp128", 16, 16),
    ] {
        check(&dl, ty, &defs, size, alignment, &[]);
    }
    check(&dl, "{ i8 }", &defs, 1, 1, &[0]);
    check(&dl, "{ i8, i64, i8 }", &defs, 16, 4, &[0, 4, 12]);
    check(&dl, "{ i8, double }", &defs, 16, 8, &[0, 8]);
}

#[test]
fn integer_alignment_uses_next_width_then_largest_width_not_largest_alignment() {
    let dl = DataLayout::parse("e-i32:128-i64:32:64-i128:64").unwrap();
    let defs = HashMap::new();
    for (ty, size, alignment) in [
        ("i7", 1, 1),
        ("i9", 2, 2),
        ("i24", 16, 16),
        ("i33", 8, 4),
        ("i64", 8, 4),
        ("i65", 16, 8),
        ("i129", 24, 8),
    ] {
        check(&dl, ty, &defs, size, alignment, &[]);
    }
}

#[test]
fn default_pointer_fallback_and_explicit_address_spaces() {
    let dl = DataLayout::parse("E-p:32:32-p1:64:128:256:32-p270:32:32").unwrap();
    let defs = struct_defs("%Node = type { i32, %Node* }");
    assert!(!dl.little_endian);
    assert_eq!(dl.pointer_size, 4);
    check(&dl, "ptr addrspace(1)", &defs, 16, 16, &[]);
    check(&dl, "ptr addrspace(270)", &defs, 4, 4, &[]);
    check(&dl, "ptr addrspace(999)", &defs, 4, 4, &[]);
    check(&dl, "i8 addrspace(1)*", &defs, 16, 16, &[]);
    check(&dl, "%Node*", &defs, 4, 4, &[]);
    check(&dl, "i8 addrspace(1)* addrspace(270)*", &defs, 4, 4, &[]);
    check(&dl, "[2 x ptr addrspace(1)]", &defs, 32, 16, &[]);
    check(&dl, "%Node", &defs, 8, 4, &[0, 4]);
    assert_eq!(dl.pointer_layout(1).unwrap().alignment, 16);
    assert!(dl.pointer_layout(1 << 24).is_err());
}

#[test]
fn pointer_representation_size_is_distinct_from_allocation_stride() {
    let dl = DataLayout::parse("e-p0:24:32:64:16").unwrap();
    assert_eq!(dl.pointer_size, 3);
    check(&dl, "ptr", &HashMap::new(), 4, 4, &[]);
    check(&dl, "[3 x ptr]", &HashMap::new(), 12, 4, &[]);
}

#[test]
fn packed_structs_still_advance_by_field_allocation_size() {
    let dl = DataLayout::parse("e-i64:64").unwrap();
    let defs = struct_defs("%Packed = type <{ i8, i64, i16 }>\n%Natural = type { i32, i8 }");
    check(&dl, "%Packed", &defs, 11, 1, &[0, 1, 9]);
    check(&dl, "<{ i8, i24, i8 }>", &defs, 6, 1, &[0, 1, 5]);
    check(&dl, "<{ i8, %Natural, i8 }>", &defs, 10, 1, &[0, 1, 9]);
    check(&dl, "{ i8, %Packed, i32 }", &defs, 16, 4, &[0, 1, 12]);
    check(&dl, "[2 x %Packed]", &defs, 22, 1, &[]);
}

#[test]
fn nested_arrays_use_allocation_stride_not_integer_store_size() {
    let dl = DataLayout::parse("e").unwrap();
    let defs = HashMap::new();
    check(&dl, "i24", &defs, 4, 4, &[]);
    check(&dl, "[3 x i24]", &defs, 12, 4, &[]);
    check(&dl, "[2 x [3 x i24]]", &defs, 24, 4, &[]);
    check(&dl, "[2 x { i32, i8 }]", &defs, 16, 4, &[]);
    check(
        &dl,
        "{ i8, [2 x { i32, i8 }], i16 }",
        &defs,
        24,
        4,
        &[0, 4, 20],
    );
    check(&dl, "[9 x i1]", &defs, 9, 1, &[]);
    check(&dl, "<{ [2 x <{i8, i16}>], i32 }>", &defs, 10, 1, &[0, 6]);
}

#[test]
fn aggregate_minimum_does_not_apply_to_packed_structs_or_scalar_arrays() {
    let dl = DataLayout::parse("e-a:128:256").unwrap();
    let defs = HashMap::new();
    check(&dl, "{ i8, i32, i8 }", &defs, 16, 16, &[0, 4, 8]);
    check(&dl, "<{ i8, i32, i8 }>", &defs, 6, 1, &[0, 1, 5]);
    check(&dl, "[2 x { i8 }]", &defs, 32, 16, &[]);
    check(&dl, "[3 x i8]", &defs, 3, 1, &[]);
    check(&dl, "{i8, {i8}, i8}", &defs, 48, 16, &[0, 16, 32]);
    check(&dl, "<{i8, {i8}, i8}>", &defs, 18, 1, &[0, 1, 17]);
    check(
        &DataLayout::parse("e-a:0:256").unwrap(),
        "{i8}",
        &defs,
        1,
        1,
        &[0],
    );
}

#[test]
fn empty_llvm_structs_and_zero_arrays_are_not_cpp_empty_objects() {
    let dl = DataLayout::parse("e-a:128").unwrap();
    let defs = struct_defs("%Empty = type {}\n%Packed = type <{}>");
    check(&dl, "%Empty", &defs, 0, 16, &[]);
    check(&dl, "%Packed", &defs, 0, 1, &[]);
    check(&dl, "[0 x i64]", &defs, 0, 4, &[]);
    check(&dl, "[100 x %Empty]", &defs, 0, 16, &[]);
    check(&dl, "{ i8, [0 x i32], i8 }", &defs, 16, 16, &[0, 4, 4]);
}

#[test]
fn explicit_float_alignment_is_not_integer_alignment() {
    let dl = DataLayout::parse("e-i32:128-f32:16:64-f64:32:128").unwrap();
    check(&dl, "float", &HashMap::new(), 4, 2, &[]);
    check(&dl, "double", &HashMap::new(), 8, 4, &[]);
    check(
        &dl,
        "{i8, float, double}",
        &HashMap::new(),
        16,
        4,
        &[0, 2, 8],
    );
    assert!(DataLayout::parse("e")
        .unwrap()
        .layout_of("x86_fp80", &HashMap::new())
        .is_err());
}

#[test]
fn endianness_is_required_unambiguous_and_does_not_change_offsets() {
    for text in ["", "p:64:64", "m:e-i64:64", "e-E", "E-e", "e-e", "e-"] {
        assert!(DataLayout::parse(text).is_err(), "accepted {text:?}");
    }
    let little = DataLayout::parse("e-i64:64").unwrap();
    let big = DataLayout::parse("E-i64:64").unwrap();
    assert!(little.little_endian);
    assert!(!big.little_endian);
    assert_eq!(
        little.layout_of("{i8,i64}", &HashMap::new()).unwrap(),
        big.layout_of("{i8,i64}", &HashMap::new()).unwrap()
    );
}

#[test]
fn quoted_layout_and_ir_directive_are_accepted_but_trailing_text_is_not() {
    for text in [" \"e-p:32:32\" ", "target datalayout = \"e-p:32:32\""] {
        assert_eq!(DataLayout::parse(text).unwrap().pointer_size, 4);
    }
    for text in [
        "\"e",
        "target datalayout = e",
        "target datalayout \"e\"",
        "e garbage",
        "target datalayout = \"e\" junk",
    ] {
        assert!(DataLayout::parse(text).is_err(), "accepted {text}");
    }
}

#[test]
fn non_object_specs_are_validated_without_affecting_pointer_or_aggregate_abi() {
    let dl = DataLayout::parse("e-m:o-n8:16:32:64-ni:1:7-S256-Fn128-P1-G2-A3-v64:32:64-v128:256")
        .unwrap();
    check(&dl, "ptr", &HashMap::new(), 8, 8, &[]);
    check(&dl, "{i8}", &HashMap::new(), 1, 1, &[0]);
    assert!(DataLayout::parse("E-Fi64-S0-a0:0:64").is_ok());
}

#[test]
fn invalid_alignment_pointer_and_unknown_layout_specs_fail_closed() {
    for spec in [
        "i0:8",
        "i32:0",
        "i32:7",
        "i32:24",
        "i32:64:32",
        "i8:16",
        "i32:32:32:32",
        "i32:",
        "i:32",
        "i16777216:32",
        "f32:0",
        "v128:24",
        "v0:64",
        "a:3",
        "a:64:0",
        "a1:64",
        "a:",
        "p:0:64",
        "p:7:8",
        "p:64:0",
        "p:64:24",
        "p:64:64:32",
        "p:64:64:64:0",
        "p:64:64:64:128",
        "p:64:64:64:7",
        "p:64",
        "p:64:64:64:64:64",
        "p16777216:64:64",
        "p-1:64:64",
        "n",
        "n8:0",
        "ni",
        "ni:0",
        "ni:16777216",
        "S24",
        "Fi7",
        "Fz64",
        "A16777216",
        "G1:32",
        "P",
        "m:unknown",
        "m:e:extra",
        "z42",
        "e ",
    ] {
        assert!(
            DataLayout::parse(&format!("e-{spec}")).is_err(),
            "accepted {spec}"
        );
    }
}

#[test]
fn invalid_or_unsupported_types_never_produce_layouts() {
    let dl = DataLayout::parse(MSVC).unwrap();
    for ty in [
        "",
        "void",
        "metadata",
        "token",
        "opaque",
        "target(\"x\")",
        "unknown*",
        "i0",
        "i-1",
        "i+8",
        "i8388608",
        "i32 junk",
        "i8, i32",
        "floatx",
        "[2 x i32",
        "[2 x i32]]",
        "[2xi32]",
        "[-1 x i8]",
        "[+1 x i8]",
        "[0 x void]",
        "{i8,}",
        "{,i8}",
        "{i8 i32}",
        "<{i8}",
        "{i8}>",
        "ptr addrspace(-1)",
        "ptr addrspace(16777216)",
        "ptr addrspace(1) junk",
        "i8 addrspace(1)",
        "ptr*",
        "void ()*",
        "%",
        "%\"unterminated",
    ] {
        assert!(dl.layout_of(ty, &HashMap::new()).is_err(), "accepted {ty}");
    }
}

#[test]
fn vector_types_are_explicitly_rejected_even_when_alignment_is_known() {
    let dl = DataLayout::parse("e-v32:32-v128:128").unwrap();
    for ty in [
        "<4 x i32>",
        "<3 x i8>",
        "<8 x i1>",
        "<vscale x 4 x i32>",
        "[0 x <4 x float>]",
    ] {
        let error = dl.layout_of(ty, &HashMap::new()).unwrap_err().to_string();
        assert!(error.contains("vector"), "{error}");
    }
}

#[test]
fn missing_named_references_and_by_value_cycles_fail() {
    let dl = DataLayout::parse("e").unwrap();
    let defs = struct_defs("%A = type { %B }\n%B = type { [0 x %A] }\n%C = type { %C }\n%D = type { %Missing }\n%Opaque = type opaque");
    for ty in [
        "%A",
        "%B",
        "%C",
        "%D",
        "%Missing",
        "%Missing*",
        "[0 x %Missing]",
        "%Opaque",
    ] {
        assert!(dl.layout_of(ty, &defs).is_err(), "accepted {ty}");
    }
    let error = format!("{:#}", dl.layout_of("%A", &defs).unwrap_err());
    assert!(error.contains("cycle"), "{error}");
    check(&dl, "%C*", &defs, 8, 8, &[]);
    assert!(dl.layout_of("%C", &defs).is_err());
}

#[test]
fn cleaned_quoted_names_preserve_escaped_spelling_and_delimiters() {
    let dl = DataLayout::parse("e").unwrap();
    let defs = struct_defs(
        r#"%"struct.X::{<a,b>}[q]\22" = type { i32 }
%"struct.Simple" = type { i8, i32 }
"#,
    );
    check(&dl, r#"%"struct.Simple""#, &defs, 8, 4, &[0, 4]);
    check(
        &dl,
        r#"{ i8, %"struct.X::{<a,b>}[q]\22", i8 }"#,
        &defs,
        12,
        4,
        &[0, 4, 8],
    );
    assert!(dl.layout_of(r#"%"bad\q0""#, &defs).is_err());
    assert!(dl.layout_of("%struct.Simple", &defs).is_ok());
}

#[test]
fn checked_arithmetic_rejects_counts_products_sums_and_bit_size_overflow() {
    let dl = DataLayout::parse("e").unwrap();
    let max = usize::MAX;
    for ty in [
        format!("[{max}0 x i8]"),
        format!("[{max} x i64]"),
        format!("[{max} x i8]"),
        format!("[8 x [{} x i64]]", max / 8),
        format!("{{ [{} x i8], i64 }}", max / 8),
    ] {
        assert!(dl.layout_of(&ty, &HashMap::new()).is_err(), "accepted {ty}");
    }
    assert!(align_up(max, 8).is_err());
    assert_eq!(align_up(max, 1).unwrap(), max);
    assert!(DataLayout::parse(&format!("e-p:{max}0:64")).is_err());
    assert!(DataLayout::parse(&format!("e-i32:{max}0")).is_err());
    check(
        &dl,
        &format!("[{} x i8]", max / 8),
        &HashMap::new(),
        max / 8,
        1,
        &[],
    );
}

#[test]
fn deeply_nested_input_and_acyclic_named_chains_return_errors_not_stack_overflows() {
    let dl = DataLayout::parse("e").unwrap();
    let deep = format!("{}i8{}", "[1 x ".repeat(256), "]".repeat(256));
    assert!(dl.layout_of(&deep, &HashMap::new()).is_err());
    let mut defs = HashMap::new();
    for n in 0..256 {
        defs.insert(
            format!("N{n}"),
            IrStructDef {
                fields: vec![if n == 255 {
                    "i8".into()
                } else {
                    format!("%N{}", n + 1)
                }],
                is_packed: false,
            },
        );
    }
    assert!(dl.layout_of("%N0", &defs).is_err());
}

#[test]
fn shared_named_subtrees_are_memoized_without_false_cycle_errors() {
    let dl = DataLayout::parse("e").unwrap();
    let mut defs = struct_defs("%N0 = type { i8 }");
    for n in 1..=24 {
        defs.insert(
            format!("N{n}"),
            IrStructDef {
                fields: vec![format!("%N{}", n - 1); 2],
                is_packed: false,
            },
        );
    }
    check(&dl, "%N24", &defs, 1 << 24, 1, &[0, 1 << 23]);
}
