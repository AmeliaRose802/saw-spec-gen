use super::*;

#[test]
fn parses_full_transition() {
    let m = StateModel::from_cli(&["this.isActive@8:1=0->1".into()]).unwrap();
    assert_eq!(m.fields.len(), 1);
    let f = &m.fields[0];
    assert_eq!(f.param, "this");
    assert_eq!(f.field, "isActive");
    assert_eq!(f.offset, 8);
    assert_eq!(f.width, 1);
    assert_eq!(f.pre.as_deref(), Some("0"));
    assert_eq!(f.post, Some(StatePost::Equals("1".into())));
}

#[test]
fn unconstrained_sides_with_star_and_empty() {
    let m = StateModel::from_cli(&["this.x@0:4=*->5".into(), "this.y@4:4=2->".into()]).unwrap();
    assert_eq!(m.fields[0].pre, None);
    assert_eq!(m.fields[0].post, Some(StatePost::Equals("5".into())));
    assert_eq!(m.fields[1].pre.as_deref(), Some("2"));
    assert_eq!(m.fields[1].post, None);
}

#[test]
fn keep_post_is_recognised() {
    let m = StateModel::from_cli(&["this.id@0:16=->keep".into()]).unwrap();
    assert_eq!(m.fields[0].pre, None);
    assert_eq!(m.fields[0].post, Some(StatePost::Keep));
}

#[test]
fn slice_expr_width_one_uses_index_op() {
    let f = StateField {
        param: "this".into(),
        field: "isActive".into(),
        offset: 8,
        width: 1,
        pre: None,
        post: None,
    };
    assert_eq!(f.slice_expr("this_pre"), "(this_pre @ 8)");
}

#[test]
fn slice_expr_wide_field_is_little_endian_join() {
    let f = StateField {
        param: "this".into(),
        field: "counter".into(),
        offset: 0,
        width: 4,
        pre: None,
        post: None,
    };
    assert_eq!(
        f.slice_expr("this_post"),
        "(join (reverse (this_post @@ [0 .. 3])))"
    );
}

#[test]
fn min_size_is_max_offset_plus_width() {
    let m = StateModel::from_cli(&[
        "this.a@0:1=->".into(),
        "this.b@8:4=->".into(),
        "this.c@4:2=->".into(),
    ])
    .unwrap();
    assert_eq!(m.min_size_for("this"), 12);
    assert_eq!(m.min_size_for("other"), 0);
}

#[test]
fn params_are_distinct_in_first_seen_order() {
    let m = StateModel::from_cli(&[
        "a.x@0:1=->".into(),
        "b.y@0:1=->".into(),
        "a.z@1:1=->".into(),
    ])
    .unwrap();
    assert_eq!(m.params(), vec!["a", "b"]);
}

#[test]
fn emit_preconditions_references_pre_var() {
    let m = StateModel::from_cli(&["this.isActive@8:1=0->1".into()]).unwrap();
    let mut out = String::new();
    m.emit_preconditions(&mut out);
    assert!(
        out.contains("llvm_precond {{ (this_pre @ 8) == 0 }};"),
        "got:\n{out}"
    );
}

#[test]
fn emit_preconditions_empty_when_no_pre() {
    let m = StateModel::from_cli(&["this.isActive@8:1=->1".into()]).unwrap();
    let mut out = String::new();
    m.emit_preconditions(&mut out);
    assert!(out.is_empty(), "expected no preconditions; got:\n{out}");
}

#[test]
fn emit_postconditions_binds_post_var_and_asserts() {
    let m = StateModel::from_cli(&["this.isActive@8:1=0->1".into()]).unwrap();
    let mut out = String::new();
    let saw = |_: &str| "llvm_array 16 (llvm_int 8)".to_string();
    m.emit_postconditions(&mut out, &saw);
    assert!(
        out.contains("this_post <- llvm_fresh_var \"this_post\" (llvm_array 16 (llvm_int 8));"),
        "got:\n{out}"
    );
    assert!(
        out.contains("llvm_points_to this_ptr (llvm_term this_post);"),
        "got:\n{out}"
    );
    assert!(
        out.contains("llvm_postcond {{ (this_post @ 8) == 1 }};"),
        "got:\n{out}"
    );
}

#[test]
fn emit_postconditions_keep_compares_to_pre() {
    let m = StateModel::from_cli(&["this.id@0:1=->keep".into()]).unwrap();
    let mut out = String::new();
    let saw = |_: &str| "llvm_array 16 (llvm_int 8)".to_string();
    m.emit_postconditions(&mut out, &saw);
    assert!(
        out.contains("llvm_postcond {{ (this_post @ 0) == (this_pre @ 0) }};"),
        "got:\n{out}"
    );
}

#[test]
fn emit_postconditions_skips_params_without_post() {
    let m = StateModel::from_cli(&["this.x@0:1=5->".into()]).unwrap();
    let mut out = String::new();
    let saw = |_: &str| "llvm_array 4 (llvm_int 8)".to_string();
    m.emit_postconditions(&mut out, &saw);
    assert!(out.is_empty(), "expected no postconditions; got:\n{out}");
}

#[test]
fn rejects_missing_separator() {
    assert!(StateModel::from_cli(&["this.x@0:1=5".into()]).is_err());
}

#[test]
fn rejects_zero_width() {
    assert!(StateModel::from_cli(&["this.x@0:0=->".into()]).is_err());
}

#[test]
fn rejects_malformed_lhs() {
    assert!(StateModel::from_cli(&["thisx@0:1=->".into()]).is_err());
    assert!(StateModel::from_cli(&["this.x:1=->".into()]).is_err());
    assert!(StateModel::from_cli(&["this.x@0=->".into()]).is_err());
}
