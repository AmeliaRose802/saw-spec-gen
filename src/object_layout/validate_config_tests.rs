use super::*;

#[test]
fn offsets_and_alignments_are_assertions_not_overrides() {
    let mut p = plan(padded());
    let mut cfg = LayoutConfig::default();
    cfg.offsets.insert("this.large".into(), 4);
    cfg.alignments.insert("this".into(), 4);
    finish(&mut p, &cfg, &[], None, &[]).unwrap();
    for path in ["this.large", "this.padding", "missing.large", "this"] {
        cfg.offsets = BTreeMap::from([(path.into(), 3)]);
        assert!(
            finish(&mut p, &cfg, &[], None, &[]).is_err(),
            "accepted {path}"
        );
    }
    cfg.offsets.clear();
    cfg.alignments.insert("this".into(), 8);
    assert!(finish(&mut p, &cfg, &[], None, &[]).is_err());
    cfg.alignments.clear();
    cfg.active_members
        .insert("missing.choice".into(), "real".into());
    assert!(finish(&mut p, &cfg, &[], None, &[]).is_err());
}

#[test]
fn modifies_expands_aggregate_prefixes_without_matching_similar_names() {
    let obj = object(
        vec![
            field("key.a", 0, 8),
            field("key.b", 1, 8),
            field("keys", 2, 8),
            field("tail", 3, 8),
        ],
        4,
        1,
    );
    let mut p = plan(obj);
    let mut cfg = LayoutConfig {
        modifies: Some(vec!["this.key".into()]),
        ..LayoutConfig::default()
    };
    finish(&mut p, &cfg, &[], None, &[]).unwrap();
    assert_eq!(p.objects["this"].framed, ["keys", "tail"]);
    for selection in [
        "this",
        "this.pad",
        "this.ke",
        "missing.key",
        "this.key.*",
        "return.key",
    ] {
        cfg.modifies = Some(vec![selection.into()]);
        assert!(
            finish(&mut p, &cfg, &[], None, &[]).is_err(),
            "accepted {selection}"
        );
    }
    cfg.modifies = Some(Vec::new());
    finish(&mut p, &cfg, &[], None, &[]).unwrap();
    assert_eq!(p.objects["this"].framed.len(), 4);
    p.objects.get_mut("this").unwrap().mutable = false;
    cfg.modifies = Some(vec!["this.key".into()]);
    assert!(finish(&mut p, &cfg, &[], None, &[]).is_err());
}

#[test]
fn pointer_mutation_and_return_frames_are_never_accepted() {
    let mut p = plan(pointer_object());
    let cfg = LayoutConfig {
        modifies: Some(vec!["this.link".into()]),
        ..LayoutConfig::default()
    };
    assert!(finish(&mut p, &cfg, &[], None, &[])
        .unwrap_err()
        .to_string()
        .contains("pointer mutation"));
    let mut ret = object(vec![field("n", 0, 32)], 8, 4);
    ret.region = "return".into();
    ret.lowering = "sret".into();
    ret.framed.push("n".into());
    let mut p = plan(ret);
    let lowered = finish_raw(&mut p, &["return.n == 0"]).unwrap();
    assert!(lowered[0].contains("result_pre"));
    assert!(p.objects["return"].framed.is_empty());
    finish(&mut p, &LayoutConfig::default(), &[], Some(4), &[]).unwrap();
    for prefix in [0, 3, 9] {
        assert!(finish(&mut p, &LayoutConfig::default(), &[], Some(prefix), &[]).is_err());
    }
}

#[test]
fn alias_assertions_check_exact_known_sizes_and_warn_for_unrelated_types() {
    let mut p = plan(padded());
    for bytes in [7, 9] {
        assert!(finish(
            &mut p,
            &LayoutConfig::default(),
            &[],
            None,
            &[format!("Test={bytes}")]
        )
        .is_err());
    }
    finish(
        &mut p,
        &LayoutConfig::default(),
        &[],
        None,
        &["%\"struct.Test\"=8".into(), "Other=99".into()],
    )
    .unwrap();
    assert!(p
        .warnings
        .iter()
        .any(|w| w.contains("Other=99") && w.contains("non-target")));
    let mut p = plan(object(vec![field("n", 0, 24)], 4, 4));
    assert!(finish(
        &mut p,
        &LayoutConfig::default(),
        &[],
        None,
        &["Word24=3".into()]
    )
    .is_err());
    finish(
        &mut p,
        &LayoutConfig::default(),
        &[],
        None,
        &["Word24=4".into()],
    )
    .unwrap();
}

#[test]
fn frame_inference_expands_arrays_preserves_pointers_and_respects_readonly() {
    let mut obj = object(
        vec![
            field("items[0]", 0, 8),
            field("items[1]", 1, 8),
            field("other", 2, 8),
        ],
        3,
        1,
    );
    obj.framed.push("items".into());
    let mut p = plan(obj);
    finish_raw(&mut p, &[]).unwrap();
    assert_eq!(p.objects["this"].framed, ["items[0]", "items[1]"]);
    let cfg = LayoutConfig {
        modifies: Some(vec!["this.items[0]".into()]),
        ..LayoutConfig::default()
    };
    finish(&mut p, &cfg, &[], None, &[]).unwrap();
    assert_eq!(p.objects["this"].framed, ["items[1]", "other"]);
    p.objects.get_mut("this").unwrap().mutable = false;
    finish_raw(&mut p, &[]).unwrap();
    assert!(p.objects["this"].framed.is_empty());
    assert!(finish(&mut p, &cfg, &[], Some(3), &[]).is_err());
}

#[test]
fn alias_assertions_for_validated_typed_pointers_do_not_need_pointee_definitions() {
    let mut obj = pointer_object();
    obj.projection = Projection::Fields;
    obj.framed.push("link".into());
    obj.layout.fields[0].llvm_type = "%Node*".into();
    validate_object(&mut obj, &format!("{IR}%Node = type {{ i32 }}")).unwrap();
    finish(
        &mut plan(obj),
        &LayoutConfig::default(),
        &[],
        None,
        &["Handle=8".into()],
    )
    .unwrap();
}
