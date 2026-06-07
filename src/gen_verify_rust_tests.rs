use super::*;
use crate::buffer_overrides::BufferOverrides;

#[test]
fn resolves_simple_unary_function() {
    let ir = "\
define internal i32 @_RNvCs1234_8mycrate7add_one(i32 %x) unnamed_addr {
entry:
  ret i32 1
}
";
    let arts = resolve_target(ir, "add_one").unwrap();
    assert_eq!(arts.mangled_name, "_RNvCs1234_8mycrate7add_one");
    assert_eq!(arts.params.len(), 1);
    assert_eq!(arts.params[0].bits, 32);
    assert!(matches!(
        arts.return_kind,
        RustReturnKind::Scalar { bits: 32 }
    ));
    assert!(arts.globals.is_empty());
}

#[test]
fn bool_arg_is_bridged_in_saw_script() {
    let ir = "\
define i32 @_RNvCs0_4test4bf32(i1 %b) {
entry:
  ret i32 0
}
";
    let arts = resolve_target(ir, "bf32").unwrap();
    assert_eq!(arts.params[0].bits, 1);
    let saw = gen_verify_rust_emit::emit_saw_script(
        "bf32",
        "bf32_spec",
        "x.bc",
        "x.cry",
        &arts,
        &BufferOverrides::default(),
        &gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(saw.contains("(x0 ! 0)"), "missing Bit bridge:\n{saw}");
}

#[test]
fn skips_drop_glue_via_signature_filter() {
    let ir = "\
define internal void @_RINvNtCs0_4core3ptr13drop_in_placeNvCs1_8mycrate7add_oneEB7_(ptr %0) {
entry:
  ret void
}
define internal i32 @_RNvCs1_8mycrate7add_one(i32 %x) {
entry:
  ret i32 1
}
";
    let arts = resolve_target(ir, "add_one").unwrap();
    assert_eq!(arts.mangled_name, "_RNvCs1_8mycrate7add_one");
}

#[test]
fn picks_up_mutable_globals_only() {
    let ir = "\
@COUNTER = internal global i32 0
@MAX = internal constant i32 42
@__imp_foo = external global ptr
define i32 @_RNvCs0_2cc3get() {
entry:
  ret i32 0
}
";
    let arts = resolve_target(ir, "get").unwrap();
    assert_eq!(arts.globals, vec!["COUNTER".to_string()]);
}

fn unary_i32_arts() -> RustVerifyArtifacts {
    let ir = "\
define internal i32 @_RNvCs1234_8mycrate7add_one(i32 %x) unnamed_addr {
entry:
  ret i32 1
}
";
    resolve_target(ir, "add_one").unwrap()
}

#[test]
fn emit_saw_script_emits_begin_proof_before_llvm_verify() {
    let arts = unary_i32_arts();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "add_one",
        "add_one_spec",
        "add_one.bc",
        "add_one.cry",
        &arts,
        &BufferOverrides::default(),
        &gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(saw.contains("print \"BEGIN_PROOF add_one\";"));
    let begin_idx = saw.find("print \"BEGIN_PROOF add_one\";").unwrap();
    let verify_idx = saw.find("llvm_verify").unwrap();
    assert!(begin_idx < verify_idx);
}

#[test]
fn emit_saw_script_emits_proved_after_llvm_verify() {
    let arts = unary_i32_arts();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "add_one",
        "add_one_spec",
        "add_one.bc",
        "add_one.cry",
        &arts,
        &BufferOverrides::default(),
        &gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(saw.contains("print \"PROVED add_one\";"));
    let verify_idx = saw.find("llvm_verify").unwrap();
    let proved_idx = saw.find("print \"PROVED add_one\";").unwrap();
    assert!(verify_idx < proved_idx);
}

#[test]
fn emit_saw_script_keeps_legacy_verified_token() {
    let arts = unary_i32_arts();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "add_one",
        "add_one_spec",
        "add_one.bc",
        "add_one.cry",
        &arts,
        &BufferOverrides::default(),
        &gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(saw.contains("VERIFIED"), "lost legacy VERIFIED token");
}

#[test]
fn resolves_aggregate_return() {
    let ir = "\
%Pair = type { i1, i1 }
define %Pair @_RNvCs0_4test9make_pair(i32 %x) {
entry:
  ret %Pair zeroinitializer
}
";
    let arts = resolve_target(ir, "make_pair").unwrap();
    match &arts.return_kind {
        RustReturnKind::Aggregate { field_bits } => {
            assert_eq!(field_bits, &[1, 1]);
        }
        other => panic!("expected Aggregate, got {other:?}"),
    }
}

#[test]
fn parses_range_attr_on_param() {
    let ir = "\
define i32 @_RNvCs0_4test8classify(i8 range(0, 3) %x) {
entry:
  ret i32 0
}
";
    let arts = resolve_target(ir, "classify").unwrap();
    assert_eq!(arts.params[0].range, Some((0, 3)));
}

#[test]
fn range_precond_emitted_in_saw_script() {
    let ir = "\
define i32 @_RNvCs0_4test8classify(i8 range(0, 3) %x) {
entry:
  ret i32 0
}
";
    let arts = resolve_target(ir, "classify").unwrap();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "classify",
        "classify_spec",
        "x.bc",
        "x.cry",
        &arts,
        &BufferOverrides::default(),
        &gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(
        saw.contains("llvm_precond {{ x0 <= (2 : [8]) }}"),
        "missing range precond:\n{saw}"
    );
}

#[test]
fn spec_only_on_missing_returns_ok() {
    let ir = "\
define i32 @_RNvCs0_4test7add_one(i32 %x) {
entry:
  ret i32 0
}
";
    let tmp = std::env::temp_dir().join("saw_spec_gen_test_spec_only");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let ll_path = tmp.join("test.ll");
    std::fs::write(&ll_path, ir).unwrap();
    let cry_path = tmp.join("spec.cry");
    std::fs::write(&cry_path, "no_such_fn : [32] -> [32]\nno_such_fn x = x").unwrap();
    let bc_path = tmp.join("test.bc");
    std::fs::write(&bc_path, b"BC").unwrap();
    let out = tmp.join("out");
    // "no_match" has no symbol in the IR
    let result = crate::gen_verify_rust::run(
        &ll_path,
        &bc_path,
        &cry_path,
        "no_such_fn",
        "no_match",
        &out,
        true,
        &BufferOverrides::default(),
        &crate::gen_verify_rust_emit::VariantMap::default(),
    );
    assert!(
        result.is_ok(),
        "spec_only_on_missing should succeed: {result:?}"
    );
    let rj = out.join("result.json");
    assert!(rj.exists(), "result.json should be written");
    let content = std::fs::read_to_string(&rj).unwrap();
    assert!(content.contains("not_attempted"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn variant_map_parse_round_trip() {
    let raw = vec!["x0=Success:0,AlreadyActive:1".to_string()];
    let vmap = gen_verify_rust_emit::VariantMap::parse_all(&raw).unwrap();
    let vs = &vmap.entries["x0"];
    assert_eq!(vs.len(), 2);
    assert_eq!(vs[0].name, "Success");
    assert_eq!(vs[0].discriminant, 0);
    assert_eq!(vs[1].name, "AlreadyActive");
    assert_eq!(vs[1].discriminant, 1);
}

#[test]
fn variant_map_param_precondition_emitted() {
    let ir = "\
define i32 @_RNvCs0_4test8classify(i8 %x0) {
entry:
  ret i32 0
}
";
    let arts = resolve_target(ir, "classify").unwrap();
    let vmap =
        gen_verify_rust_emit::VariantMap::parse_all(&["x0=A:0,B:1,C:3".to_string()]).unwrap();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "classify",
        "classify_spec",
        "x.bc",
        "x.cry",
        &arts,
        &BufferOverrides::default(),
        &vmap,
    );
    assert!(
        saw.contains("x0 == (0 : [8]) \\/ x0 == (1 : [8]) \\/ x0 == (3 : [8])"),
        "missing variant membership precondition:\n{saw}"
    );
}

#[test]
fn variant_map_return_narrowing_two_variants() {
    let ir = "\
define i8 @_RNvCs0_4test5check(i32 %x0) {
entry:
  ret i8 0
}
";
    let arts = resolve_target(ir, "check").unwrap();
    let vmap =
        gen_verify_rust_emit::VariantMap::parse_all(&["return=Success:0,Failure:1".to_string()])
            .unwrap();
    let saw = gen_verify_rust_emit::emit_saw_script(
        "check",
        "check_spec",
        "x.bc",
        "x.cry",
        &arts,
        &BufferOverrides::default(),
        &vmap,
    );
    // Two-variant return: should emit VariantRemap bridge adapter
    assert!(
        saw.contains("if (check_spec x0) == (0 : [8])"),
        "missing VariantRemap condition:\n{saw}"
    );
    assert!(
        saw.contains("then (0 : [8])"),
        "missing then discriminant:\n{saw}"
    );
    assert!(
        saw.contains("else (1 : [8])"),
        "missing else discriminant:\n{saw}"
    );
}
