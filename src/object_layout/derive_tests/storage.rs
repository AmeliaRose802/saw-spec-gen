use super::*;

#[test]
fn unions_require_exact_selection_and_label_unused_capacity_separately() {
    let ir = "%union.U = type { i32, [12 x i8] }\n%struct.Host = type { i8, %union.U, i8 }";
    let records = vec![
        record(
            "union U",
            16,
            4,
            vec![member("int", "small", 0), member("char[16]", "large", 0)],
        ),
        record(
            "struct Host",
            24,
            4,
            vec![
                member("char", "tag", 0),
                member("union U", "choice", 4),
                member("bool", "tail", 20),
            ],
        ),
    ];
    let f = facts(ir, records);
    let mut config = LayoutConfig::default();
    assert!(derive(&f, ir, "Host", "this", &config)
        .unwrap_err()
        .to_string()
        .contains("active_members"));
    config
        .active_members
        .insert("this.choice".into(), "nonexistent".into());
    assert!(derive(&f, ir, "Host", "this", &config).is_err());
    config
        .active_members
        .insert("this.choice".into(), "small".into());
    let layout = derive(&f, ir, "Host", "this", &config).unwrap();
    assert_eq!(field(&layout, "choice.small").offset, 4);
    assert!(layout.padding.iter().any(|r| r.offset == 8
        && r.size == 12
        && r.reason.starts_with("inactive union storage (selected")));
    assert!(!layout
        .padding
        .iter()
        .any(|r| r.reason == "compiler padding" && r.offset < 20 && r.offset + r.size > 8));
    config
        .active_members
        .insert("this.choice".into(), "large".into());
    assert!(derive(&f, ir, "Host", "this", &config).is_err()); // Cannot split an i32 leaf into four guessed i8 fields.
    config.active_members.insert("this".into(), "small".into());
    assert_eq!(
        derive(&f, ir, "U", "this", &config).unwrap().fields[0].path,
        "small"
    );
}

#[test]
fn bitfields_use_exact_integer_storage_and_keep_crossing_storage_unresolved() {
    let ir = "%struct.Bits = type { i32 }";
    let mut flags = member("unsigned int", "flags", 1);
    flags.bit_offset = Some(2);
    flags.bit_width = Some(3);
    let mut barrier = member("unsigned int", "", 4);
    barrier.bit_width = Some(0);
    let layout = checked(
        ir,
        vec![record("struct Bits", 4, 4, vec![flags, barrier])],
        "Bits",
    )
    .unwrap();
    let flags = field(&layout, "flags");
    assert_eq!(
        (flags.offset, flags.size, flags.bit_offset, flags.bit_width),
        (0, 4, Some(10), Some(3))
    );
    assert_eq!(flags.llvm_type, "i32");
    assert_eq!(layout.fields.len(), 1);
    assert!(layout.unresolved.is_empty());
    assert!(layout
        .validation
        .iter()
        .any(|note| note.contains("Clang byte 1, bit Some(2)")));
    assert!(layout
        .validation
        .iter()
        .any(|note| note.contains("zero-width bitfield")));
    assert!(layout.padding.is_empty());
    let ir = "%struct.Bits = type { [4 x i8] }";
    let mut spanning = member("unsigned int", "spanning", 0);
    spanning.bit_offset = Some(4);
    spanning.bit_width = Some(12);
    let layout = checked(
        ir,
        vec![record("struct Bits", 4, 1, vec![spanning])],
        "Bits",
    )
    .unwrap();
    assert_eq!(field(&layout, "spanning").size, 0);
    assert!(field(&layout, "spanning").llvm_type.is_empty());
    assert_eq!(layout.unresolved.len(), 1);
}

#[test]
fn guarded_selected_union_keeps_record_padding_distinct_from_inactive_capacity() {
    let ir = "%struct.Small = type { i32, i8 }\n%union.U = type { %struct.Small, [8 x i8] }\n%\"class.std::optional<U>\" = type { %union.U, i8 }";
    let records = vec![
        record(
            "struct Small",
            8,
            4,
            vec![member("int", "n", 0), member("bool", "b", 4)],
        ),
        record(
            "union U",
            16,
            4,
            vec![
                member("struct Small", "small", 0),
                member("char[16]", "large", 0),
            ],
        ),
        record(
            "class std::optional<U>",
            20,
            4,
            vec![
                member("union U", "_Value", 0),
                member("bool", "_Has_value", 16),
            ],
        ),
    ];
    let mut f = facts(ir, records);
    let mut config = LayoutConfig::default();
    assert!(derive(&f, ir, "std::optional<U>", "this", &config).is_err());
    config
        .active_members
        .insert("this.value".into(), "small".into());
    let layout = derive(&f, ir, "std::optional<U>", "this", &config).unwrap();
    assert_eq!(
        field(&layout, "value.small.b").guard.as_deref(),
        Some("has_value")
    );
    assert!(layout
        .padding
        .iter()
        .any(|r| r.offset == 5 && r.size == 3 && r.reason == "compiler padding"));
    assert!(layout.padding.iter().any(|r| r.offset == 8
        && r.size == 8
        && r.reason.contains("inactive union storage")
        && r.reason.contains("guard has_value")));
    f.records.get_mut("U").unwrap().size = 12;
    assert!(derive(&f, ir, "std::optional<U>", "this", &config).is_err());
}
