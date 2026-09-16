use super::*;
use crate::object_layout::{clang, validate, LayoutPlan, ObjectPlan, Projection};

use crate::object_layout::emit_bitfields as emission;
#[path = "bitfield_emit_tests.rs"]
mod emit_tests;

const DL: &str = "e-p:64:64-i64:64-i128:128";
const IR: &str = "target datalayout = \"e-p:64:64-i64:64-i128:128\"\n";

fn member(name: &str, start: usize, width: usize) -> RecordMember {
    RecordMember {
        name: name.into(),
        source_type: "unsigned int".into(),
        offset: start / 8,
        bit_offset: (width != 0).then_some(start % 8),
        bit_width: Some(width),
        is_base: false,
        is_empty: false,
        children: Vec::new(),
    }
}

fn facts(body: &str, members: Vec<RecordMember>) -> (CompilerLayouts, String) {
    let ir = format!("{IR}%struct.Bits = type {body}");
    let allocated = DataLayout::parse(DL)
        .unwrap()
        .layout_of("%struct.Bits", &struct_defs(&ir))
        .unwrap();
    let record = RecordLayout {
        source_type: "struct Bits".into(),
        size: allocated.size,
        alignment: allocated.alignment,
        is_union: false,
        members,
    };
    (
        CompilerLayouts {
            irgen_types: Default::default(),
            schema_version: 1,
            target_triple: "x86_64-pc-windows-msvc".into(),
            data_layout: DL.into(),
            compiler: "bitfield unit fixture".into(),
            command: Vec::new(),
            llvm_types: BTreeMap::from([("struct.Bits".into(), body.into())]),
            records: BTreeMap::from([("Bits".into(), record)]),
        },
        ir,
    )
}

fn layout(body: &str, members: Vec<RecordMember>) -> Result<ObjectLayout> {
    let (facts, ir) = facts(body, members);
    derive(&facts, &ir, "Bits", "this", &LayoutConfig::default())
}

fn object(body: &str, members: Vec<RecordMember>) -> ObjectPlan {
    let layout = layout(body, members).unwrap();
    let mut object = ObjectPlan {
        region: "this".into(),
        asserted: layout.fields.iter().map(|f| f.path.clone()).collect(),
        layout,
        projection: Projection::Fields,
        mutable: true,
        argument_index: 0,
        lowering: "pointer".into(),
        configured_shape: None,
        inferred_shape: None,
        framed: Vec::new(),
        selectors: BTreeMap::new(),
    };
    validate::validate_object(&mut object, IR).unwrap();
    object
}

fn pair() -> ObjectPlan {
    let mut high = member("hi", 5, 3);
    high.source_type = "signed int".into();
    object("{ i8 }", vec![member("lo", 0, 3), high])
}

fn field<'a>(object: &'a mut ObjectPlan, path: &str) -> &'a mut FieldLayout {
    object
        .layout
        .fields
        .iter_mut()
        .find(|f| f.path == path)
        .unwrap()
}

fn guarded_pair() -> ObjectPlan {
    let mut flag = member("has_value", 8, 0);
    flag.source_type = "bool".into();
    flag.bit_width = None;
    let mut object = object(
        "{ i8, i8 }",
        vec![member("lo", 0, 3), member("hi", 5, 3), flag],
    );
    field(&mut object, "lo").guard = Some("has_value".into());
    field(&mut object, "hi").guard = Some("has_value".into());
    validate::validate_object(&mut object, IR).unwrap();
    object
}

#[test]
fn extracted_bitfields_omit_padding_declarations_and_label_the_named_union_gaps() {
    let dump = "*** Dumping AST Record Layout\n         0 | struct Bits\n     0:0-2 |   unsigned int lo\n     0:3-4 |   unsigned int \n     0:5-7 |   int hi\n       4:- |   unsigned int \n           | [sizeof=4, align=4]\n";
    let (mut facts, ir) = facts("{ i32 }", Vec::new());
    facts.records = clang::parse_record_layouts(dump).unwrap();
    let layout = derive(&facts, &ir, "Bits", "this", &LayoutConfig::default()).unwrap();
    assert!(layout.unresolved.is_empty());
    assert_eq!(
        layout
            .fields
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["hi", "lo"]
    );
    assert!(layout
        .fields
        .iter()
        .all(|f| f.offset == 0 && f.size == 4 && f.llvm_type == "i32"));
    assert!(layout.padding.is_empty());
    for note in [
        "unnamed bitfield",
        "zero-width bitfield",
        "bits 3..5 unasserted",
        "bits 8..32 unasserted",
    ] {
        assert!(
            layout.validation.iter().any(|v| v.contains(note)),
            "missing {note}"
        );
    }
    assert!(layout.fields.iter().all(|f| f.validity.is_none())); // Signed is a bitvector too.
}

#[test]
fn unnamed_backing_words_are_reserved_but_zero_width_barriers_occupy_no_bits() {
    let layout = layout(
        "{ i32, i32 }",
        vec![member("lo", 0, 3), member("", 32, 1), member("", 64, 0)],
    )
    .unwrap();
    assert_eq!(layout.fields.len(), 1);
    assert!(layout.padding.is_empty());
    assert!(layout.unresolved.is_empty());
    let empty = self::layout("{ i8 }", vec![member("", 0, 0)]).unwrap();
    assert!(empty.fields.is_empty());
    assert_eq!((empty.padding[0].offset, empty.padding[0].size), (0, 1));
}

#[test]
fn derivation_rejects_overlapping_named_bits_even_when_path_order_hides_them() {
    let error = layout(
        "{ i8 }",
        vec![member("a", 0, 4), member("m", 6, 2), member("z", 2, 1)],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("overlapping semantic fields"), "{error}");
    let mut scalar = member("whole", 0, 0);
    scalar.bit_width = None;
    assert!(layout("{ i32 }", vec![scalar, member("part", 7, 1)]).is_err());
}

#[test]
fn unsupported_mapping_keeps_the_named_field_and_fails_closed() {
    for (body, start, width) in [
        ("{ [4 x i8] }", 4, 12),
        ("{ i9 }", 9, 1),
        ("{ float }", 0, 3),
        ("{ ptr }", 0, 3),
        ("{ i136 }", 0, 129),
    ] {
        let layout = layout(body, vec![member("value", start, width)]).unwrap();
        assert_eq!(layout.fields.len(), 1);
        assert_eq!(layout.unresolved.len(), 1, "accepted {body}");
        assert!(layout.unresolved[0].contains("refusing unsupported bitfield emission"));
    }
    let (mut facts, ir) = facts("{ i32 }", vec![member("value", 0, 3)]);
    facts.data_layout = DL.replacen('e', "E", 1);
    let ir = ir.replacen(DL, &facts.data_layout, 1);
    let layout = derive(&facts, &ir, "Bits", "this", &LayoutConfig::default()).unwrap();
    assert!(layout.unresolved[0].contains("little-endian"));
}

#[test]
fn validation_accepts_shared_words_and_rejects_bit_or_storage_overlap() {
    for projection in [Projection::Bytes, Projection::Fields] {
        let mut object = pair();
        object.projection = projection;
        object.layout.fields.reverse();
        validate::validate_object(&mut object, IR).unwrap();
        field(&mut object, "hi").bit_offset = Some(2);
        assert!(validate::validate_object(&mut object, IR)
            .unwrap_err()
            .to_string()
            .contains("overlapping"));
    }
    let mut object = pair();
    object.layout.size = 3;
    for field in &mut object.layout.fields {
        field.llvm_type = "i16".into();
        field.size = 2;
        field.bit_offset = Some(0);
    }
    field(&mut object, "hi").offset = 1;
    assert!(validate::validate_object(&mut object, IR)
        .unwrap_err()
        .to_string()
        .contains("overlapping"));
    let mut object = pair();
    object.layout.size = 2;
    field(&mut object, "hi").llvm_type = "i16".into();
    field(&mut object, "hi").size = 2;
    assert!(validate::validate_object(&mut object, IR)
        .unwrap_err()
        .to_string()
        .contains("overlapping"));
}

#[test]
fn invalid_bit_metadata_and_pointer_float_or_wide_storage_are_rejected() {
    for (start, width) in [
        (None, Some(3)),
        (Some(0), None),
        (Some(0), Some(0)),
        (Some(7), Some(2)),
        (Some(0), Some(9)),
        (Some(usize::MAX), Some(1)),
    ] {
        let mut object = pair();
        let value = field(&mut object, "lo");
        value.bit_offset = start;
        value.bit_width = width;
        assert!(validate::validate_object(&mut object, IR).is_err());
    }
    for ty in ["ptr", "float", "double", "i0", "i129", "[1 x i8]"] {
        let mut object = pair();
        let value = field(&mut object, "lo");
        value.llvm_type = ty.into();
        value.runtime = true; // Runtime storage cannot bypass bitfield checks.
        assert!(
            validate::validate_object(&mut object, IR).is_err(),
            "accepted {ty}"
        );
    }
    for source in ["bool *", "const float", "long double"] {
        let mut object = pair();
        field(&mut object, "lo").source_type = source.into();
        assert!(validate::validate_object(&mut object, IR).is_err());
    }
    let mut enum_object = pair();
    field(&mut enum_object, "lo").source_type = "enum Holder<int *>::Kind".into();
    validate::validate_object(&mut enum_object, IR).unwrap();
    let mut object = pair();
    field(&mut object, "lo").size = 2;
    assert!(validate::validate_object(&mut object, IR).is_err());
    assert!(validate::validate_object(&mut pair(), "target datalayout = \"E\"").is_err());
}

#[test]
fn padding_cannot_overlap_unasserted_bits_and_guards_must_agree_per_word() {
    let mut object = object("{ i32 }", vec![member("value", 0, 3)]);
    object.layout.padding.push(ByteRange {
        offset: 1,
        size: 1,
        reason: "not named bits".into(),
    });
    assert!(validate::validate_object(&mut object, IR)
        .unwrap_err()
        .to_string()
        .contains("padding overlaps"));
    let mut object = guarded_pair();
    field(&mut object, "hi").guard = None;
    assert!(validate::validate_object(&mut object, IR)
        .unwrap_err()
        .to_string()
        .contains("guards disagree"));
}

#[test]
fn explicit_and_inferred_nonbyte_shapes_reject_bitfields_but_byte_extents_work() {
    for inferred in [false, true] {
        let mut object = pair();
        if inferred {
            object.inferred_shape = Some("llvm_int 8".into());
        } else {
            object.configured_shape = Some("llvm_int 8".into());
        }
        let original = serde_json::to_value(&object).unwrap();
        assert!(validate::validate_object(&mut object, IR)
            .unwrap_err()
            .to_string()
            .contains("bitfields in legacy"));
        assert_eq!(serde_json::to_value(&object).unwrap(), original);
    }
    for projection in [Projection::Bytes, Projection::Fields, Projection::Llvm] {
        let mut object = pair();
        object.projection = projection.clone();
        object.configured_shape = Some("llvm_array 1 (llvm_int 8)".into());
        validate::validate_object(&mut object, IR).unwrap();
        assert_eq!(
            object.projection,
            if projection == Projection::Fields {
                Projection::Fields
            } else {
                Projection::Bytes
            }
        );
    }
}

#[test]
fn alias_sizes_never_treat_a_bitfield_backing_word_as_its_source_size() {
    let object = pair(); // unsigned int fields packed into i8, not sizeof(int)=1.
    let mut plan = LayoutPlan {
        data_layout: DL.into(),
        objects: BTreeMap::from([("this".into(), object)]),
        ..LayoutPlan::default()
    };
    validate::finish(
        &mut plan,
        &LayoutConfig::default(),
        &[],
        None,
        &["unsigned int=4".into()],
    )
    .unwrap();
    assert!(plan
        .warnings
        .iter()
        .any(|w| w.contains("no compiler-grounded size assertion")));
}
