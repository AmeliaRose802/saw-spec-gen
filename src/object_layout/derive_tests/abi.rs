use super::*;

#[test]
fn nested_fields_explicit_padding_and_base_hierarchy_are_deterministic() {
    let ir = "%struct.Base = type { i32 }\n%struct.Inner = type { i16, i8 }\n%struct.Outer = type { %struct.Base, i8, [3 x i8], %struct.Inner, ptr }";
    let parent = base(children(
        member("struct Base", "", 0),
        vec![member("int", "id", 0)],
    ));
    let inner = children(
        member("struct Inner", "inner", 8),
        vec![member("short", "n", 8), member("bool", "live", 10)],
    );
    let records = vec![
        record("struct Base", 4, 4, parent.children.clone()),
        record(
            "struct Inner",
            4,
            2,
            vec![member("short", "n", 0), member("bool", "live", 2)],
        ),
        record(
            "struct Outer",
            24,
            8,
            vec![
                parent.clone(),
                member("char", "tag", 4),
                inner,
                member("void *", "link", 16),
            ],
        ),
    ];
    let layout = checked(ir, records.clone(), "Outer").unwrap();
    assert_eq!(field(&layout, "inner.n").offset, 8);
    assert_eq!(
        field(&layout, "inner.live").validity.as_deref(),
        Some("bool")
    );
    assert_eq!(
        (
            field(&layout, "link").size,
            field(&layout, "link").llvm_type.as_str()
        ),
        (8, "ptr")
    );
    assert!(field(&layout, "link").is_pointer);
    assert_eq!(layout.bases, vec![parent]);
    assert_eq!(
        layout.padding,
        vec![
            ByteRange {
                offset: 5,
                size: 3,
                reason: "compiler padding".into()
            },
            ByteRange {
                offset: 11,
                size: 5,
                reason: "compiler padding".into()
            },
        ]
    );
    assert_eq!(
        serde_json::to_value(&layout).unwrap(),
        serde_json::to_value(checked(ir, records, "Outer").unwrap()).unwrap()
    );
}

#[test]
fn sizes_alignments_offsets_and_scalar_types_are_cross_checked() {
    let ir = "%struct.S = type { i8, i32 }";
    let good = record(
        "struct S",
        8,
        4,
        vec![member("char", "a", 0), member("int", "b", 4)],
    );
    for (size, align) in [(4, 4), (8, 3), (8, 2), (8, 16)] {
        let mut bad = good.clone();
        bad.size = size;
        bad.alignment = align;
        assert!(
            checked(ir, vec![bad], "S").is_err(),
            "accepted sizeof={size} align={align}"
        );
    }
    for offset in [1, 2, 3, 5, 8, usize::MAX] {
        let mut bad = good.clone();
        bad.members[1].offset = offset;
        assert!(
            checked(ir, vec![bad], "S").is_err(),
            "accepted offset {offset}"
        );
    }
    let mut bad = good;
    bad.members[1].source_type = "float".into();
    assert!(checked(ir, vec![bad], "S").is_err());
    let duplicate = record(
        "struct S",
        8,
        4,
        vec![member("char", "same", 0), member("int", "same", 4)],
    );
    assert!(checked(ir, vec![duplicate], "S")
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
}

#[test]
fn alignas_may_exceed_llvm_abi_alignment_without_guessing_storage() {
    let ir = "%struct.Aligned = type { i32, [12 x i8] }";
    let layout = checked(
        ir,
        vec![record(
            "struct Aligned",
            16,
            16,
            vec![member("int", "value", 0)],
        )],
        "Aligned",
    )
    .unwrap();
    assert_eq!((layout.alignment, layout.llvm_alignment), (16, 4));
    assert_eq!((layout.padding[0].offset, layout.padding[0].size), (4, 12));
    let packed = "%struct.Packed = type <{ i8, i64 }>";
    assert!(checked(
        packed,
        vec![record(
            "struct Packed",
            9,
            1,
            vec![member("char", "c", 0), member("long long", "n", 1)]
        )],
        "Packed"
    )
    .is_ok());
}

#[test]
fn source_mapping_is_qualified_tag_normalized_and_uniquely_suffixed() {
    let ir = "%\"class.ns::Box<struct ns::Item>.7\" = type { i32 }";
    let r = record(
        "class ns::Box<class ns::Item>",
        4,
        4,
        vec![member("int", "n", 0)],
    );
    let layout = checked(ir, vec![r.clone()], "ns::Box<struct ns::Item>").unwrap();
    assert_eq!(layout.llvm_type, "class.ns::Box<struct ns::Item>.7");
    let ambiguous = format!("{ir}\n%\"struct.ns::Box<ns::Item>\" = type {{ i32 }}");
    assert!(checked(&ambiguous, vec![r.clone()], "ns::Box<ns::Item>")
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    assert!(checked(
        "%struct.Unrelated = type { i32 }",
        vec![r.clone()],
        "ns::Box<ns::Item>"
    )
    .is_err());
    assert!(checked(ir, vec![r.clone()], "Box<ns::Item>").is_err());
    let mut f = facts(ir, vec![r.clone()]);
    f.records.insert("duplicate key".into(), r);
    assert!(derive(
        &f,
        ir,
        "ns::Box<ns::Item>",
        "this",
        &LayoutConfig::default()
    )
    .is_err());
}

#[test]
fn stale_equal_sized_ir_and_cross_target_facts_are_rejected() {
    let ir = "%struct.S = type { i32 }";
    let f = facts(
        ir,
        vec![record("struct S", 4, 4, vec![member("Word", "value", 0)])],
    );
    assert!(derive(
        &f,
        "%struct.S = type { float }",
        "S",
        "this",
        &LayoutConfig::default()
    )
    .is_err());
    let different_target = format!("target triple = \"aarch64-unknown-linux-gnu\"\n{ir}");
    assert!(derive(&f, &different_target, "S", "this", &LayoutConfig::default()).is_err());
}

#[test]
fn unknown_typedef_and_enum_keep_exact_bits_and_pointer_types() {
    let ir = "%struct.S = type { i16, i32, ptr }";
    let layout = checked(
        ir,
        vec![record(
            "struct S",
            16,
            8,
            vec![
                member("Word", "word", 0),
                member("enum class Color", "color", 4),
                member("Handle", "handle", 8),
            ],
        )],
        "S",
    )
    .unwrap();
    assert_eq!(
        (
            field(&layout, "word").llvm_type.as_str(),
            field(&layout, "word").size
        ),
        ("i16", 2)
    );
    assert_eq!(field(&layout, "color").validity, None);
    assert!(field(&layout, "handle").is_pointer);
    assert_eq!(field(&layout, "handle").llvm_type, "ptr");
    assert!(layout
        .validation
        .iter()
        .any(|note| note.contains("exact LLVM leaf") && note.contains("Word")));
    assert!(layout.unresolved.is_empty());
}

#[test]
fn pointers_and_long_follow_target_not_host_abi() {
    for (triple, dl, ir, size, alignment, long_offset) in [
        (
            "i686-pc-windows-msvc",
            "e-p:32:32-i64:64",
            "%struct.S = type { i8*, i32 }",
            8,
            4,
            4,
        ),
        (
            "x86_64-unknown-linux-gnu",
            DL,
            "%struct.S = type { ptr, i64 }",
            16,
            8,
            8,
        ),
        (
            "x86_64-pc-windows-msvc",
            DL,
            "%struct.S = type { ptr, i32 }",
            16,
            8,
            8,
        ),
    ] {
        let mut f = facts(
            ir,
            vec![record(
                "struct S",
                size,
                alignment,
                vec![
                    member("char *", "p", 0),
                    member("unsigned long", "n", long_offset),
                ],
            )],
        );
        f.target_triple = triple.into();
        f.data_layout = dl.into();
        let layout = derive(&f, ir, "S", "this", &LayoutConfig::default()).unwrap();
        assert_eq!(
            (
                field(&layout, "p").size,
                field(&layout, "p").llvm_type.as_str()
            ),
            (long_offset, "ptr")
        );
    }
    let ir = "%struct.P = type { [2 x ptr], i8 }";
    let layout = checked(
        ir,
        vec![record(
            "struct P",
            24,
            8,
            vec![member("int *[2]", "p", 0), member("bool", "b", 16)],
        )],
        "P",
    )
    .unwrap();
    assert_eq!(field(&layout, "p[1]").offset, 8);
    assert!(field(&layout, "p[0]").is_pointer && field(&layout, "p[1]").is_pointer);
}

#[test]
fn arrays_resolve_record_templates_and_preserve_multidimensional_indices() {
    let ir =
        "%struct.E = type { i16, i8 }\n%struct.R = type { i8, [2 x %struct.E], [2 x [2 x i16]] }";
    let e = record(
        "struct E",
        4,
        2,
        vec![member("short", "n", 0), member("bool", "b", 2)],
    );
    let r = record(
        "struct R",
        18,
        2,
        vec![
            member("char", "tag", 0),
            member("struct E[2]", "items", 2),
            member("short[2][2]", "grid", 10),
        ],
    );
    let layout = checked(ir, vec![e, r.clone()], "R").unwrap();
    assert_eq!(field(&layout, "items[0].b").offset, 4);
    assert_eq!(field(&layout, "items[1].b").offset, 8);
    assert_eq!(field(&layout, "grid[1][1]").offset, 16);
    assert_eq!(layout.fields.len(), 9);
    let mut with_template = r.clone();
    with_template.members[1].children = vec![member("short", "n", 2), member("bool", "b", 4)];
    let expanded = checked(ir, vec![with_template], "R").unwrap();
    assert_eq!(field(&expanded, "items[1].n").offset, 6);
    assert!(checked(ir, vec![r], "R").is_err());
}

#[test]
fn unknown_array_elements_use_llvm_allocation_stride_not_store_size() {
    let ir = "%struct.S = type { [3 x i24] }";
    let layout = checked(
        ir,
        vec![record(
            "struct S",
            12,
            4,
            vec![member("Word[3]", "words", 0)],
        )],
        "S",
    )
    .unwrap();
    assert_eq!(
        (
            field(&layout, "words[1]").offset,
            field(&layout, "words[1]").size
        ),
        (4, 3)
    );
    assert_eq!(
        layout
            .padding
            .iter()
            .map(|r| (r.offset, r.size))
            .collect::<Vec<_>>(),
        vec![(3, 1), (7, 1), (11, 1)]
    );
}

#[test]
fn by_value_source_cycles_and_unsupported_pointer_storage_fail_closed() {
    let ir = "%struct.S = type { i8 }";
    assert!(checked(
        ir,
        vec![record(
            "struct S",
            1,
            1,
            vec![member("struct S", "again", 0)]
        )],
        "S"
    )
    .is_err());
    let ir = "%struct.S = type { ptr addrspace(1) }";
    assert!(checked(
        ir,
        vec![record("struct S", 8, 8, vec![member("void *", "p", 0)])],
        "S"
    )
    .is_err());
}
