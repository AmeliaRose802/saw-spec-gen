use super::super::model::{ByteRange, ObjectLayout};
use super::*;
use std::collections::BTreeMap;

#[path = "validate_config_tests.rs"]
mod config;
#[path = "validate_preconditions_tests.rs"]
mod preconditions;

const DL: &str = "e-p:64:64-i64:64-i128:128";
const IR: &str = "target datalayout = \"e-p:64:64-i64:64-i128:128\"\n";

fn field(path: &str, offset: usize, bits: usize) -> FieldLayout {
    FieldLayout {
        path: path.into(),
        source_type: format!("Word{bits}"),
        llvm_type: format!("i{bits}"),
        offset,
        size: bits.div_ceil(8),
        bit_offset: None,
        bit_width: None,
        array_count: None,
        array_stride: None,
        guard: None,
        validity: None,
        is_pointer: false,
        runtime: false,
    }
}

fn object(fields: Vec<FieldLayout>, size: usize, alignment: usize) -> ObjectPlan {
    ObjectPlan {
        region: "this".into(),
        projection: Projection::Bytes,
        mutable: true,
        argument_index: 0,
        lowering: "indirect".into(),
        configured_shape: None,
        inferred_shape: None,
        asserted: fields.iter().map(|f| f.path.clone()).collect(),
        framed: Vec::new(),
        selectors: BTreeMap::new(),
        layout: ObjectLayout {
            allocation_type: String::new(),
            source_type: "struct Test".into(),
            llvm_type: "struct.Test".into(),
            size,
            alignment,
            llvm_alignment: alignment,
            fields,
            padding: Vec::new(),
            bases: Vec::new(),
            validation: Vec::new(),
            unresolved: Vec::new(),
        },
    }
}

fn padded() -> ObjectPlan {
    let mut obj = object(vec![field("small", 0, 8), field("large", 4, 32)], 8, 4);
    obj.layout.padding.push(ByteRange {
        offset: 1,
        size: 3,
        reason: "compiler padding".into(),
    });
    obj
}

fn optional() -> ObjectPlan {
    let mut obj = object(
        vec![
            field("key.value.isActive", 0, 8),
            field("key.has_value", 4, 8),
        ],
        8,
        4,
    );
    for field in &mut obj.layout.fields {
        field.validity = Some("bool".into());
    }
    obj.layout.fields[0].guard = Some("key.has_value".into());
    obj
}

fn plan(object: ObjectPlan) -> LayoutPlan {
    LayoutPlan {
        data_layout: DL.into(),
        objects: BTreeMap::from([(object.region.clone(), object)]),
        ..LayoutPlan::default()
    }
}

fn finish_raw(plan: &mut LayoutPlan, raw: &[&str]) -> Result<Vec<String>> {
    finish(
        plan,
        &LayoutConfig::default(),
        &raw.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        None,
        &[],
    )
}

fn shape_error(mut obj: ObjectPlan, shape: &str, expected: &str) {
    obj.configured_shape = Some(shape.into());
    let before = serde_json::to_value(&obj).unwrap();
    let error = format!("{:#}", validate_object(&mut obj, IR).unwrap_err());
    assert!(error.contains(expected), "{error}");
    assert_eq!(serde_json::to_value(&obj).unwrap(), before);
}

#[test]
fn byte_extent_is_exact_in_both_directions() {
    for size in [0, 1, 7, 9, 32] {
        shape_error(
            padded(),
            &format!("llvm_array {size} (llvm_int 8)"),
            "compiler size is exactly 8",
        );
    }
    let mut obj = padded();
    obj.configured_shape = Some(" (llvm_array 8 ((llvm_int 8))) ".into());
    validate_object(&mut obj, IR).unwrap();
    assert_eq!(obj.projection, Projection::Bytes);
    assert!(obj.selectors.is_empty());
}

#[test]
fn natural_padding_is_not_an_extra_value() {
    let mut obj = padded();
    obj.configured_shape = Some("llvm_struct_type [llvm_int 8, llvm_int 32]".into());
    validate_object(&mut obj, IR).unwrap();
    assert_eq!(obj.projection, Projection::Llvm);
    assert_eq!(obj.selectors["small"], "$.0");
    assert_eq!(obj.selectors["large"], "$.1");
    assert_eq!(
        field_expr(&obj, &obj.layout.fields[1], "this_pre"),
        "this_pre.1"
    );
    shape_error(
        padded(),
        "llvm_struct_type [llvm_int 8, llvm_array 3 (llvm_int 8), llvm_int 32]",
        "value-bearing padding",
    );
    shape_error(
        padded(),
        "llvm_struct_type [llvm_int 8]",
        "compiler size is exactly",
    );
}

#[test]
fn equal_size_shapes_cannot_omit_or_reinterpret_fields() {
    shape_error(
        padded(),
        "llvm_packed_struct_type [llvm_int 64]",
        "does not match semantic field",
    );
    shape_error(
        padded(),
        "llvm_array 2 (llvm_int 32)",
        "does not match semantic field",
    );
    shape_error(
        object(vec![field("word", 0, 32)], 4, 4),
        "llvm_array 2 (llvm_int 16)",
        "does not match semantic field",
    );
    shape_error(
        object(vec![field("bit", 0, 1)], 1, 1),
        "llvm_int 8",
        "does not match semantic field",
    );
    let mut obj = object(vec![field("first", 0, 8), field("last", 4, 8)], 8, 4);
    obj.configured_shape = Some("llvm_struct_type [llvm_int 8]".into());
    let ir = "target datalayout = \"e-a:64\"";
    assert!(validate_object(&mut obj, ir)
        .unwrap_err()
        .to_string()
        .contains("alignment"));
    obj.layout.alignment = 8;
    assert!(validate_object(&mut obj, ir)
        .unwrap_err()
        .to_string()
        .contains("omits semantic fields"));
}

#[test]
fn packed_structs_and_underaligned_shapes_use_target_offsets() {
    let mut obj = object(vec![field("tag", 0, 8), field("word", 1, 64)], 9, 1);
    obj.configured_shape = Some("llvm_packed_struct_type [llvm_int 8, llvm_int 64]".into());
    validate_object(&mut obj, IR).unwrap();
    assert_eq!(obj.selectors["word"], "$.1");
    let mut aligned = object(
        vec![
            field("a", 0, 32),
            field("b", 4, 32),
            field("c", 8, 32),
            field("d", 12, 32),
        ],
        16,
        16,
    );
    aligned.layout.llvm_alignment = 4;
    aligned.configured_shape = Some("llvm_array 4 (llvm_int 32)".into());
    validate_object(&mut aligned, IR).unwrap();
    assert_eq!(aligned.selectors["c"], "($ @ 2)");
}

#[test]
fn target_data_layout_controls_scalar_stride_not_host_abi() {
    let mut obj = object(vec![field("a", 0, 24), field("b", 4, 24)], 8, 4);
    obj.configured_shape = Some("llvm_array 2 (llvm_int 24)".into());
    validate_object(&mut obj, IR).unwrap();
    assert_eq!(obj.selectors["b"], "($ @ 1)");
    let mut scalar = object(vec![field("word", 0, 24)], 4, 4);
    scalar.configured_shape = Some("llvm_int 24".into());
    validate_object(&mut scalar, IR).unwrap();
    assert_eq!(scalar.selectors["word"], "$");
}

#[test]
fn recursive_named_struct_and_array_selectors_parenthesize_array_access() {
    let ir = format!("{IR}%\"struct.ns::Leaf\" = type {{ i16, i16 }}\n%struct.Outer = type {{ [3 x %\"struct.ns::Leaf\"] }}");
    let fields: Vec<_> = (0..3)
        .flat_map(|i| {
            [
                field(&format!("items[{i}].a"), i * 4, 16),
                field(&format!("items[{i}].b"), i * 4 + 2, 16),
            ]
        })
        .collect();
    for shape in [
        "llvm_alias \"struct.Outer\"",
        "llvm_struct \"struct.Outer\"",
        "llvm_struct_type [llvm_array 3 (llvm_struct_type [llvm_int 16, llvm_int 16])]",
    ] {
        let mut obj = object(fields.clone(), 12, 2);
        obj.configured_shape = Some(shape.into());
        validate_object(&mut obj, &ir).unwrap();
        assert_eq!(obj.selectors["items[2].b"], "($.0 @ 2).1");
        assert_eq!(field_expr(&obj, &obj.layout.fields[5], "x"), "(x.0 @ 2).1");
        assert_eq!(
            finish_raw(&mut plan(obj), &["this.items[2].b == 3"]).unwrap()[0],
            "((this_pre.0 @ 2).1 == 3)"
        );
    }
}

#[test]
fn malformed_unresolved_and_cyclic_shapes_fail_before_emission() {
    for shape in [
        "llvm_int 0",
        "llvm_int 8388608",
        "llvm_int 8 trailing",
        "llvm_int8",
        "llvm_array 4 llvm_int 8",
        "llvm_array -1 (llvm_int 8)",
        "llvm_struct_type [llvm_int 32,]",
        "llvm_alias \"missing\"",
        "llvm_alias \"unterminated",
        "llvm_float",
    ] {
        let mut obj = object(vec![field("n", 0, 32)], 4, 4);
        obj.configured_shape = Some(shape.into());
        assert!(validate_object(&mut obj, IR).is_err(), "accepted {shape}");
    }
    let mut obj = object(vec![field("n", 0, 32)], 4, 4);
    obj.configured_shape = Some("llvm_alias \"Cycle\"".into());
    assert!(validate_object(&mut obj, &format!("{IR}%Cycle = type {{ %Cycle }}")).is_err());
    assert!(validate_object(&mut padded(), "").is_err());
    assert!(validate_object(&mut padded(), &format!("{IR}{IR}")).is_err());
}

#[test]
fn duplicate_paths_colliding_keys_overlap_and_incomplete_bitfields_are_rejected() {
    for (paths, expected) in [
        (["same", "same"], "duplicate"),
        (["a.b", "a__b"], "field_key collision"),
        (["a[0]", "a_0"], "field_key collision"),
        (["a", "a.b"], "scalar and aggregate"),
    ] {
        let mut obj = object(vec![field(paths[0], 0, 8), field(paths[1], 1, 8)], 2, 1);
        assert!(validate_object(&mut obj, IR)
            .unwrap_err()
            .to_string()
            .contains(expected));
    }
    let mut obj = object(vec![field("a", 0, 32), field("b", 2, 16)], 4, 4);
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("overlapping"));
    obj = padded();
    obj.layout.fields[0].bit_offset = Some(0);
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("bitfield"));
    obj.layout.fields[0].bit_width = Some(3);
    validate_object(&mut obj, IR).unwrap();
    obj.layout.fields[0].bit_offset = None;
    obj.layout.fields[0].bit_width = None;
    obj.layout.unresolved.push("unresolved storage".into());
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("unresolved"));
}

#[test]
fn floating_point_is_never_silently_reinterpreted_as_integer_bits() {
    for (ty, source) in [
        ("float", "Alias"),
        ("double", "Alias"),
        ("i32", "const float"),
        ("i64", "long double"),
    ] {
        let mut obj = object(vec![field("value", 0, 64)], 8, 8);
        obj.layout.fields[0].llvm_type = ty.into();
        obj.layout.fields[0].source_type = source.into();
        assert!(validate_object(&mut obj, IR)
            .unwrap_err()
            .to_string()
            .contains("floating-point"));
    }
}

fn pointer_object() -> ObjectPlan {
    let mut obj = object(vec![field("link", 0, 64), field("count", 8, 32)], 16, 8);
    obj.layout.fields[0].llvm_type = "ptr".into();
    obj.layout.fields[0].source_type = "Handle".into();
    obj.layout.fields[0].is_pointer = true;
    obj
}

#[test]
fn only_framed_fields_projection_can_contain_pointer_leaves() {
    let mut obj = pointer_object();
    assert!(validate_object(&mut obj, IR).is_err());
    obj.projection = Projection::Fields;
    assert!(validate_object(&mut obj, IR).is_err());
    obj.framed.push("link".into());
    validate_object(&mut obj, IR).unwrap();
    obj.configured_shape = Some("llvm_array 16 (llvm_int 8)".into());
    validate_object(&mut obj, IR).unwrap();
    assert_eq!(obj.projection, Projection::Fields);
    obj.configured_shape = Some("llvm_struct \"Ptr\"".into());
    assert!(validate_object(&mut obj, &format!("{IR}%Ptr = type {{ ptr, i32 }}")).is_err());
    let mut p = plan(pointer_object());
    finish_raw(&mut p, &[]).unwrap();
    assert_eq!(p.objects["this"].framed, ["link"]);
    assert!(finish_raw(&mut p, &["this.link == 0"]).is_err());
}

#[test]
fn zero_length_arrays_do_not_hide_unsupported_legacy_types() {
    let mut obj = object(vec![field("word", 0, 64)], 8, 8);
    obj.configured_shape = Some("llvm_alias \"Bad\"".into());
    for ty in ["ptr", "double"] {
        let error = validate_object(&mut obj, &format!("{IR}%Bad = type {{ i64, [0 x {ty}] }}"))
            .unwrap_err();
        assert!(error.to_string().contains("cannot freshen"));
    }
}

#[test]
fn legacy_optionals_fail_closed_and_byte_projection_is_little_endian_only() {
    shape_error(
        optional(),
        "llvm_struct_type [llvm_int 8, llvm_int 32]",
        "guarded optionals",
    );
    let mut obj = padded();
    assert!(validate_object(&mut obj, "target datalayout = \"E\"").is_err());
    obj.projection = Projection::Fields;
    validate_object(&mut obj, "target datalayout = \"E\"").unwrap();
}

#[test]
fn scalar_metadata_bounds_and_padding_conflicts_are_checked() {
    for (size, offset, ty) in [
        (3, 4, "i32"),
        (4, 5, "i32"),
        (4, usize::MAX, "i32"),
        (4, 4, "[4 x i8]"),
        (4, 4, ""),
    ] {
        let mut obj = padded();
        obj.layout.fields[1].size = size;
        obj.layout.fields[1].offset = offset;
        obj.layout.fields[1].llvm_type = ty.into();
        assert!(validate_object(&mut obj, IR).is_err());
    }
    let mut obj = padded();
    obj.layout.padding[0].size = 4;
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("padding overlaps"));
    obj = padded();
    obj.layout.fields[0].is_pointer = true;
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("pointer/type mismatch"));
}
