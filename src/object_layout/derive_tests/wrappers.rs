use super::*;

#[test]
fn optional_wrappers_normalize_all_supported_compiler_spellings_and_guards() {
    for (flag, value) in [
        ("_Has_value", "_Value"),
        ("_M_engaged", "_M_value"),
        ("__engaged_", "__val_"),
    ] {
        let (ir, records) = optional_fixture(flag, value);
        let layout = checked(&ir, records.clone(), "Host").unwrap();
        assert_eq!(field(&layout, "opt.has_value").offset, 12);
        assert_eq!(field(&layout, "opt.value.id").offset, 4);
        assert_eq!(
            field(&layout, "opt.value.live").guard.as_deref(),
            Some("opt.has_value")
        );
        assert_eq!(
            field(&layout, "opt.value.live").validity.as_deref(),
            Some("bool")
        );
        assert!(layout
            .fields
            .iter()
            .all(|f| !f.path.contains(value) && !f.path.contains(flag)));
        let root = checked(&ir, records, "std::optional<Value>").unwrap();
        assert_eq!(field(&root, "has_value").offset, 8);
        assert_eq!(field(&root, "value.id").guard.as_deref(), Some("has_value"));
        assert!(root.unresolved.is_empty());
    }
}

#[test]
fn optional_unknown_or_ambiguous_representation_is_rejected() {
    let (ir, mut records) = optional_fixture("_Has_value", "_Value");
    let optional = records
        .iter_mut()
        .find(|r| normalize_name(&r.source_type).starts_with("std::optional<"))
        .unwrap();
    optional.members[0]
        .children
        .push(member("bool", "_M_engaged", 9));
    assert!(checked(&ir, records, "Host").is_err());
    let (ir, records) = optional_fixture("unknown_flag", "_Value");
    assert!(checked(&ir, records, "Host").is_err());
    let (ir, records) = optional_fixture("_Has_value", "unknown_value");
    assert!(checked(&ir, records, "Host").is_err());
}

#[test]
fn nested_optional_bool_validity_keeps_both_engagement_guards() {
    let ir = "%\"class.std::optional<bool>\" = type { i8, i8 }\n%\"class.std::optional<std::optional<bool>>\" = type { %\"class.std::optional<bool>\", i8 }";
    let inner = record(
        "class std::optional<bool>",
        2,
        1,
        vec![
            member("remove_cv_t<bool>", "_Value", 0),
            member("bool", "_Has_value", 1),
        ],
    );
    let outer = record(
        "class std::optional<std::optional<bool>>",
        3,
        1,
        vec![
            member("class std::optional<bool>", "_M_value", 0),
            member("bool", "_M_engaged", 2),
        ],
    );
    let layout = checked(ir, vec![inner, outer], "std::optional<std::optional<bool>>").unwrap();
    assert_eq!(
        field(&layout, "value.has_value").guard.as_deref(),
        Some("has_value")
    );
    let payload = field(&layout, "value.value");
    assert_eq!(
        payload.guard.as_deref(),
        Some("has_value && value.has_value")
    );
    assert_eq!(payload.validity.as_deref(), Some("bool"));
    assert_eq!(payload.source_type, "remove_cv_t<bool>");
}

#[test]
fn mutex_ambiguous_source_names_fall_back_to_typed_storage() {
    let ir = "%union.Slots = type { ptr }\n%\"class.std::mutex\" = type { %union.Slots }";
    let slots = children(
        member("union Slots", "slots", 0),
        vec![member("void *", "a", 0), member("void *", "b", 0)],
    );
    let layout = checked(
        ir,
        vec![record("class std::mutex", 8, 8, vec![slots])],
        "std::mutex",
    )
    .unwrap();
    let storage = field(&layout, "storage_0");
    assert!(storage.is_pointer);
    assert_eq!((storage.llvm_type.as_str(), storage.size), ("ptr", 8));
}

#[test]
fn msvc_shaped_mutex_and_optional_preserve_runtime_pointers_and_union_tail() {
    let (mut ir, mut records) = optional_fixture("_Has_value", "_Value");
    ir.push_str("\n%struct.CS = type { ptr, ptr }\n%union.Mtx = type { %struct.CS, [48 x i8] }\n%struct.Mtx = type { i32, %union.Mtx, i32, i32 }\n%\"class.std::mutex\" = type { %struct.Mtx }\n%struct.Both = type { i8, %\"class.std::mutex\", %\"class.std::optional<Value>\" }");
    let cs = children(
        member("struct CS", "_Critical_section", 8),
        vec![
            member("void *", "_Unused", 8),
            member("_Smtx_t", "_M_srw_lock", 16),
        ],
    );
    let alternate = children(
        member("union AlignmentStorage", "_Cs_storage", 8),
        vec![member("double", "_Val", 8), member("char[64]", "_Pad", 8)],
    );
    let storage = children(
        member("struct Mtx", "_Mtx_storage", 0),
        vec![
            member("int", "_Type", 0),
            children(member("union Mtx", "", 8), vec![cs, alternate]),
            member("long", "_Thread_id", 72),
            member("int", "_Count", 76),
        ],
    );
    let mutex = base(children(
        member("class std::_Mutex_base", "", 0),
        vec![storage],
    ));
    records.push(record("class std::mutex", 80, 8, vec![mutex]));
    records.push(record(
        "struct Both",
        104,
        8,
        vec![
            member("char", "tag", 0),
            member("class std::mutex", "mu_", 8),
            member("class std::optional<Value>", "entry", 88),
        ],
    ));
    let layout = checked(&ir, records, "Both").unwrap();
    assert_eq!(field(&layout, "mu_._Count").offset, 84);
    assert_eq!(field(&layout, "mu_._Thread_id").llvm_type, "i32");
    assert!(
        field(&layout, "mu_._Unused").is_pointer && field(&layout, "mu_._M_srw_lock").is_pointer
    );
    let runtime: Vec<_> = layout
        .fields
        .iter()
        .filter(|f| f.path.starts_with("mu_."))
        .collect();
    assert_eq!(runtime.iter().map(|f| f.size).sum::<usize>(), 76); // Only the natural 4-byte alignment gap is padding.
    assert_eq!(
        runtime
            .iter()
            .filter(|f| f.offset >= 32 && f.offset < 80)
            .map(|f| f.size)
            .sum::<usize>(),
        48
    );
    assert_eq!(field(&layout, "entry.has_value").offset, 96);
    assert!(layout
        .validation
        .iter()
        .any(|v| v.contains("opaque") && v.contains("no synchronization execution")));
    assert!(layout.unresolved.is_empty());
}

#[test]
fn linux_shaped_mutex_is_opaque_but_custom_union_types_are_not() {
    let ir = "%struct.Data = type { i32, i32, ptr }\n%union.Pthread = type { %struct.Data, [8 x i8] }\n%\"class.std::mutex\" = type { %union.Pthread }";
    let data = children(
        member("struct Data", "__data", 0),
        vec![
            member("int", "__lock", 0),
            member("unsigned int", "__count", 4),
            member("void *", "__owner", 8),
        ],
    );
    let union = children(
        member("union Pthread", "_M_mutex", 0),
        vec![
            data,
            member("char[24]", "__size", 0),
            member("long", "__align", 0),
        ],
    );
    let mut f = facts(
        ir,
        vec![record("class std::mutex", 24, 8, vec![union.clone()])],
    );
    f.target_triple = "x86_64-unknown-linux-gnu".into();
    let layout = derive(&f, ir, "std::mutex", "mu", &LayoutConfig::default()).unwrap();
    assert!(field(&layout, "__owner").is_pointer);
    assert_eq!(
        layout.fields.iter().map(|field| field.size).sum::<usize>(),
        24
    );
    let custom_ir = ir.replace("class.std::mutex", "class.custom::mutex");
    let custom = facts(
        &custom_ir,
        vec![record("class custom::mutex", 24, 8, vec![union])],
    );
    assert!(derive(
        &custom,
        &custom_ir,
        "custom::mutex",
        "mu",
        &LayoutConfig::default()
    )
    .unwrap_err()
    .to_string()
    .contains("active_members"));
}

#[test]
fn mutex_requires_an_actual_nested_named_subobject_not_an_equal_sized_blob() {
    let ir = "%\"class.std::mutex\" = type { i32 }\n%struct.Holder = type { i32 }";
    let records = vec![
        record("class std::mutex", 4, 4, vec![member("int", "_Count", 0)]),
        record(
            "struct Holder",
            4,
            4,
            vec![member("class std::mutex", "mu_", 0)],
        ),
    ];
    assert!(checked(ir, records, "Holder")
        .unwrap_err()
        .to_string()
        .contains("exact nested LLVM"));
}
