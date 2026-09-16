use super::*;

#[test]
fn named_bool_validity_is_guarded_locally_and_originals_are_kept() {
    let mut p = plan(optional());
    let raw = "valid(this.key.value.isActive) && this.key.has_value is valid";
    let lowered = finish_raw(&mut p, &[raw]).unwrap();
    assert_eq!(
        lowered[0],
        "(if ((this_pre @ 4) == 1) then ((this_pre @ 0) <= 1) else True) && ((this_pre @ 4) <= 1)"
    );
    assert_eq!(p.semantic_preconditions, [raw]);
    assert!(p.warnings.is_empty());
    let lowered = finish_raw(
        &mut p,
        &["this.key.value.isActive == 0 && this.key.has_value == 1"],
    )
    .unwrap();
    assert_eq!(
        lowered[0],
        "(if ((this_pre @ 4) == 1) then ((this_pre @ 0) == 0) else True) && ((this_pre @ 4) == 1)"
    );
}

#[test]
fn nested_guards_resolve_transitively_and_cycles_or_nonbool_flags_fail() {
    let mut obj = optional();
    obj.layout.fields.push(field("outer", 7, 8));
    obj.layout.fields[2].validity = Some("bool".into());
    obj.layout.fields[1].guard = Some("outer".into());
    validate_object(&mut obj, IR).unwrap();
    let lowered = finish_raw(&mut plan(obj.clone()), &["valid(this.key.value.isActive)"]).unwrap();
    assert!(lowered[0].contains("(this_pre @ 7) == 1"));
    assert!(lowered[0].contains("(this_pre @ 4) == 1"));
    obj.layout.fields[2].guard = Some("key.has_value".into());
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("cyclic"));
    obj = optional();
    obj.layout.fields[1].validity = None;
    assert!(validate_object(&mut obj, IR)
        .unwrap_err()
        .to_string()
        .contains("non-bool"));
}

#[test]
fn byte_field_extraction_uses_store_size_and_little_endian_order() {
    let obj = padded();
    assert_eq!(
        field_expr(&obj, &obj.layout.fields[1], "x"),
        "(join (reverse (take`{4} (drop`{4} x))))"
    );
    let mut p = plan(obj);
    p.objects.get_mut("this").unwrap().mutable = false;
    assert_eq!(
        finish_raw(&mut p, &["this.large == 0"]).unwrap()[0],
        "((join (reverse (take`{4} (drop`{4} this)))) == 0)"
    );
}

#[test]
fn named_fields_use_exact_tokens_and_ignore_quoted_literals_and_comments() {
    let mut obj = object(
        vec![field("a", 0, 8), field("ab", 1, 8), field("items[0]", 2, 8)],
        3,
        1,
    );
    obj.projection = Projection::Fields;
    let mut p = plan(obj);
    let raw =
        "this.a == this.ab && this.items[0] == 0 && \"this.a\" == \"this.ab\" /* this.padding */";
    let result = finish_raw(&mut p, &[raw]).unwrap();
    assert_eq!(result[0], "(this_pre.a == this_pre.ab) && (this_pre.items_0 == 0) && \"this.a\" == \"this.ab\" /* this.padding */");
    for raw in [
        "this.abc == 0",
        "this.items == 0",
        "this.items[1] == 0",
        "valid(missing.a)",
        "this.a is unknown",
    ] {
        assert!(finish_raw(&mut p, &[raw]).is_err(), "accepted {raw}");
    }
}

#[test]
fn enum_comparisons_do_not_invent_enumerator_restrictions() {
    let mut obj = object(vec![field("kind", 0, 32)], 4, 4);
    obj.layout.fields[0].source_type = "enum Kind".into();
    let mut p = plan(obj);
    finish_raw(&mut p, &["this.kind == 17"]).unwrap();
    assert!(p.validity_constraints.is_empty());
    assert!(finish_raw(&mut p, &["valid(this.kind)"]).is_err());
}

#[test]
fn numeric_indices_reject_padding_and_bounds_but_identify_real_field_bytes() {
    let mut p = plan(padded());
    for raw in [
        "(this_pre @ 1) == 0",
        "this @ 3 == 0",
        "this @ 8 == 0",
        "this_pre @ 9999999999999999999999999999 == 0",
    ] {
        assert!(finish_raw(&mut p, &[raw]).is_err(), "accepted {raw}");
    }
    let result = finish_raw(&mut p, &["(this @ 6) == 0", "((this_pre)) @ (0) <= 1"]).unwrap();
    assert_eq!(result[0], "(this_pre @ 6) == 0");
    assert!(p.warnings[0].contains("byte 2 of actual field this.large"));
    assert!(p.warnings[0].contains("Intended field cannot be inferred"));
    let count = p.warnings.len();
    finish_raw(&mut p, &["(this @ 6) == 0", "((this_pre)) @ (0) <= 1"]).unwrap();
    assert_eq!(p.warnings.len(), count);
}

#[test]
fn dynamic_slices_and_transformed_object_indices_fail_closed() {
    for raw in [
        "this_pre @ i == 0",
        "this_pre @ (4 + n) == 0",
        "this_pre @ 4 + n == 0",
        "this_pre @@ 0 == 0",
        "take`{1} (drop`{1} this_pre) == [0]",
        "take`{1} this_pre == [0]",
        "f this_pre @ 0 == 0",
        "f (this_pre) @ 0 == 0",
        "(f this_pre) @ 0 == 0",
        "this_pre == other",
        "0 + this_pre @ 0 == 0",
        "this_pre @ (0x1) == 0",
    ] {
        assert!(
            finish_raw(&mut plan(padded()), &[raw]).is_err(),
            "accepted {raw}"
        );
    }
    assert!(finish_raw(&mut plan(padded()), &["f (this_pre @ 0) == 0"]).is_ok());
    assert!(finish_raw(&mut plan(padded()), &["other @ n == 0"]).is_ok());
}

#[test]
fn failed_finish_is_atomic_and_prestate_name_collisions_are_diagnosed() {
    let mut p = plan(padded());
    let before = serde_json::to_value(&p).unwrap();
    assert!(finish_raw(&mut p, &["this.large == 0", "this @ 1 == 0"]).is_err());
    assert_eq!(serde_json::to_value(&p).unwrap(), before);
    let mut other = padded();
    other.region = "this_pre".into();
    p.objects.insert(other.region.clone(), other);
    assert!(finish_raw(&mut p, &[])
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
}

#[test]
fn substitutions_preserve_application_and_arithmetic_precedence() {
    let mut obj = padded();
    obj.projection = Projection::Fields;
    for (input, expected) in [
        ("f this.small == 0", "f this_pre.small == 0"),
        ("1 + this.small == 0", "1 + this_pre.small == 0"),
        (
            "this.small + this.large == 0",
            "this_pre.small + this_pre.large == 0",
        ),
        ("other::this.small == 0", "other::this.small == 0"),
        ("f (this.small == 0)", "f ((this_pre.small == 0))"),
        ("[this.small] == [0]", "[this_pre.small] == [0]"),
    ] {
        assert_eq!(
            finish_raw(&mut plan(obj.clone()), &[input]).unwrap()[0],
            expected
        );
    }
    for input in [
        "f this.key.value.isActive == 0",
        "1 + this.key.value.isActive == 0",
        "this.key.value.isActive + 1 == 0",
        "valid(this.key)",
    ] {
        assert!(
            finish_raw(&mut plan(optional()), &[input]).is_err(),
            "accepted {input}"
        );
    }
}

#[test]
fn quoted_escapes_comments_and_non_target_aliases_do_not_require_object_facts() {
    let mut p = plan(optional());
    let raw = r#""escaped\" this.key.value.isActive" == "x" /* outer /* this @ i */ comment */"#;
    assert_eq!(finish_raw(&mut p, &[raw]).unwrap()[0], raw);
    assert!(finish_raw(&mut p, &["/* unclosed"]).is_err());
    assert!(finish_raw(&mut p, &["\"unclosed"]).is_err());
    let mut empty = LayoutPlan::default();
    finish(
        &mut empty,
        &LayoutConfig::default(),
        &[],
        None,
        &["Unrelated=123".into()],
    )
    .unwrap();
    assert!(empty.warnings[0].contains("Unrelated=123"));
}
