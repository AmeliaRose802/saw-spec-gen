//! Tests for [`super`] — extracted to keep the parent under the
//! 500 non-whitespace-line limit.

use super::*;

#[test]
fn parses_in_and_out_buffers() {
    let ov = BufferOverrides::from_cli(
        &["m=4".into(), "b=4".into()],
        &["out=10".into(), "this=auto".into()],
        &["out=canonicalize_lp_post".into()],
        &["nm=4".into(), "nb=4".into()],
        &[
            "canonicalize_lp_ret=nm,nb".into(),
            "canonicalize_lp_post=nm,m,nb,b,@pre.out".into(),
        ],
        &["canonicalize_lp_pre".into()],
        &[],
    )
    .unwrap();

    assert_eq!(ov.in_buffers["m"], "llvm_array 4 (llvm_int 8)");
    assert_eq!(ov.out_buffers["out"], "llvm_array 10 (llvm_int 8)");
    assert!(ov.out_buffer_auto.contains("this"));
    assert_eq!(ov.cryptol_fn_out["out"], "canonicalize_lp_post");
    assert_eq!(ov.cryptol_fn_pre(), Some("canonicalize_lp_pre"));
    assert_eq!(
        ov.max_len_preconds,
        vec![("nm".into(), 4), ("nb".into(), 4)]
    );
    assert_eq!(ov.value_var_for("out"), "out_pre");
    assert_eq!(ov.value_var_for("m"), "m");

    let post_args = ov.cryptol_call_args("canonicalize_lp_post").unwrap();
    assert_eq!(post_args, vec!["nm", "m", "nb", "b", "out_pre"]);

    let ret_args = ov.cryptol_call_args("canonicalize_lp_ret").unwrap();
    assert_eq!(ret_args, vec!["nm", "nb"]);

    assert_eq!(
        ov.override_saw_type("m"),
        Some("llvm_array 4 (llvm_int 8)".into())
    );
    assert_eq!(
        ov.override_saw_type("out"),
        Some("llvm_array 10 (llvm_int 8)".into())
    );
    assert_eq!(ov.override_saw_type("nm"), None);
}

#[test]
fn cryptol_fn_out_requires_matching_out_buffer_param() {
    let err =
        BufferOverrides::from_cli(&[], &[], &["out=foo".into()], &[], &[], &[], &[]).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("--out-buffer-param"), "msg = {msg}");
}

#[test]
fn rejects_malformed_value() {
    let err =
        BufferOverrides::from_cli(&["m=nine".into()], &[], &[], &[], &[], &[], &[]).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("--in-buffer-size"), "msg = {msg}");
}

#[test]
fn empty_when_no_flags() {
    let ov = BufferOverrides::from_cli(&[], &[], &[], &[], &[], &[], &[]).unwrap();
    assert!(ov.in_buffers.is_empty());
    assert!(ov.out_buffers.is_empty());
    assert!(ov.out_buffer_auto.is_empty());
    assert!(ov.cryptol_fn_out.is_empty());
    assert!(ov.max_len_preconds.is_empty());
    assert!(ov.cryptol_arg_orders.is_empty());
    assert!(ov.cryptol_fn_pre.is_none());
    assert!(ov.raw_preconds.is_empty());
    assert_eq!(ov.cryptol_call_args("anything"), None);
    assert_eq!(ov.override_saw_type("anything"), None);
    assert_eq!(ov.value_var_for("m"), "m");
}

#[test]
fn rejects_multiple_cryptol_fn_pre_flags() {
    let err = BufferOverrides::from_cli(
        &[],
        &[],
        &[],
        &[],
        &[],
        &["pre_a".into(), "pre_b".into()],
        &[],
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("--cryptol-fn-pre"),
        "msg = {err:#}"
    );
}

#[test]
fn stores_raw_preconditions_verbatim() {
    let ov = BufferOverrides::from_cli(
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[
            "(this_pre @ 128) <= 1".into(),
            "  ".into(),
            "engaged".into(),
        ],
    )
    .unwrap();
    // Blank/whitespace-only entries are dropped; others kept verbatim.
    assert_eq!(
        ov.raw_preconds,
        vec!["(this_pre @ 128) <= 1".to_string(), "engaged".to_string()]
    );
}

#[test]
fn parses_typed_wide_buffer_shapes() {
    // iW form: a single wide scalar field allocates as llvm_int W.
    // NxiW form: a homogeneous array of wide fields.
    let ov = BufferOverrides::from_cli(
        &["hdr=2xi16".into()],
        &["n=i32".into(), "arr=4xi32".into()],
        &["n=inc_post".into(), "arr=arr_post".into()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(
        ov.override_saw_type("hdr"),
        Some("llvm_array 2 (llvm_int 16)".into())
    );
    assert_eq!(ov.override_saw_type("n"), Some("llvm_int 32".into()));
    assert_eq!(
        ov.override_saw_type("arr"),
        Some("llvm_array 4 (llvm_int 32)".into())
    );
    assert!(ov.is_out_buffer("n"));
    assert!(ov.is_out_buffer("arr"));
    assert!(ov.has_in_buffer_size("hdr"));
}

#[test]
fn parses_struct_buffer_shapes() {
    let ov = BufferOverrides::from_cli(
        &["hdr=struct:PacketHeader".into()],
        &["key={16xi8,i8}".into(), "pk=<{i64,i8}>".into()],
        &["key=key_post".into(), "pk=pk_post".into()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(
        ov.override_saw_type("hdr"),
        Some("llvm_struct \"struct.PacketHeader\"".into())
    );
    assert_eq!(
        ov.override_saw_type("key"),
        Some("llvm_struct_type [llvm_array 16 (llvm_int 8), llvm_int 8]".into())
    );
    assert_eq!(
        ov.override_saw_type("pk"),
        Some("llvm_packed_struct_type [llvm_int 64, llvm_int 8]".into())
    );
    assert!(ov.is_out_buffer("key"));
    assert!(ov.is_out_buffer("pk"));
    assert!(ov.has_in_buffer_size("hdr"));
}

#[test]
fn rejects_empty_struct_shape() {
    let err =
        BufferOverrides::from_cli(&["m={}".into()], &[], &[], &[], &[], &[], &[]).unwrap_err();
    assert!(
        format!("{err:#}").contains("at least one field"),
        "msg = {err:#}"
    );
    let err =
        BufferOverrides::from_cli(&["m=struct:".into()], &[], &[], &[], &[], &[], &[]).unwrap_err();
    assert!(
        format!("{err:#}").contains("non-empty LLVM type name"),
        "msg = {err:#}"
    );
}

#[test]
fn rejects_bad_typed_buffer_shapes() {
    // Missing `i` on the element width.
    let err =
        BufferOverrides::from_cli(&[], &["n=4x32".into()], &[], &[], &[], &[], &[]).unwrap_err();
    assert!(
        format!("{err:#}").contains("--out-buffer-param"),
        "msg = {err:#}"
    );
    // Zero width is rejected.
    let err =
        BufferOverrides::from_cli(&["m=i0".into()], &[], &[], &[], &[], &[], &[]).unwrap_err();
    assert!(
        format!("{err:#}").contains("greater than 0"),
        "msg = {err:#}"
    );
}
