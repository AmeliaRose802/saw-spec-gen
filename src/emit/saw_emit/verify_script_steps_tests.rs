//! Tests for [`super::verify_script_steps`].
//!
//! Kept in a sibling file so that `verify_script_steps.rs` stays under
//! the 500-non-whitespace-line repository limit.

use super::super::verify_script_sret::SretPrestate;
use super::*;
use crate::buffer_overrides::BufferOverrides;
use crate::emit::saw_emit::cryptol_bridge::cryptol_return_for;

#[path = "verify_script_steps_stateful_tests.rs"]
mod stateful_tests;

#[test]
fn cryptol_arg_for_wraps_bool_with_index_zero() {
    let arg = cryptol_arg_for("dateValid", &TypeInfo::Bool);
    assert_eq!(arg, "(dateValid ! 0)");
}

#[test]
fn cryptol_arg_for_passes_through_integers() {
    let arg = cryptol_arg_for("count", &TypeInfo::UnsignedInt(32));
    assert_eq!(arg, "count");
}

#[test]
fn cryptol_arg_for_wraps_pointer_to_bool() {
    let ty = TypeInfo::Pointer(Box::new(TypeInfo::Bool));
    let arg = cryptol_arg_for("flag", &ty);
    assert_eq!(arg, "(flag ! 0)");
}

#[test]
fn cryptol_arg_for_does_not_wrap_pointer_to_int() {
    let ty = TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32)));
    let arg = cryptol_arg_for("p", &ty);
    assert_eq!(arg, "p");
}

#[test]
fn cryptol_return_for_wraps_bool_return_as_singleton_bitvector() {
    let call = cryptol_return_for("authenticate x y z", &TypeInfo::Bool);
    assert_eq!(call, "[authenticate x y z] : [1]");
}

#[test]
fn cryptol_return_for_passes_through_integer_return() {
    let call = cryptol_return_for("compute x", &TypeInfo::SignedInt(64));
    assert_eq!(call, "compute x");
}

#[test]
fn cryptol_return_for_passes_through_void_return() {
    let call = cryptol_return_for("do_thing", &TypeInfo::Void);
    assert_eq!(call, "do_thing");
}

/// Integration-shaped test: drive `emit_postcondition_and_close` with a
/// boolean return type and assert the emitted SAWScript wraps the call.
#[test]
fn emit_postcondition_wraps_bool_return() {
    let mut out = String::new();
    emit_postcondition_and_close(
        &mut out,
        "authenticate",
        &[
            "(dateValid ! 0)".to_string(),
            "(signatureValid ! 0)".to_string(),
            "(claimsValid ! 0)".to_string(),
        ],
        &[
            "llvm_term dateValid".to_string(),
            "llvm_term signatureValid".to_string(),
            "llvm_term claimsValid".to_string(),
        ],
        &PostconditionCtx {
            sub_callee_specs: &[],
            return_type: &TypeInfo::Bool,
            is_sret: false,
            return_bridge: None,
        },
        &BufferOverrides::default(),
    );

    assert!(
        out.contains(
            "llvm_return (llvm_term {{ [authenticate (dateValid ! 0) (signatureValid ! 0) (claimsValid ! 0)] : [1] }});"
        ),
        "expected bool return to be wrapped as [..] : [1]; got:\n{out}"
    );
}

/// Counter-example: integer return should NOT be bracketed.
#[test]
fn emit_postcondition_does_not_wrap_int_return() {
    let mut out = String::new();
    emit_postcondition_and_close(
        &mut out,
        "compute",
        &["x".to_string()],
        &["llvm_term x".to_string()],
        &PostconditionCtx {
            sub_callee_specs: &[],
            return_type: &TypeInfo::SignedInt(32),
            is_sret: false,
            return_bridge: None,
        },
        &BufferOverrides::default(),
    );

    assert!(
        out.contains("llvm_return (llvm_term {{ compute x }});"),
        "expected raw call for non-bool return; got:\n{out}"
    );
    assert!(
        !out.contains(": [1]"),
        "non-bool return should not get [..] : [1] wrapping; got:\n{out}"
    );
}

/// sret return: the postcondition must bind into `*result_ptr` via
/// `llvm_points_to`, NOT emit `llvm_return` (the LLVM function
/// returns void when sret is in play).
#[test]
fn emit_postcondition_uses_points_to_for_sret_return() {
    let mut out = String::new();
    emit_postcondition_and_close(
        &mut out,
        "getStatus",
        &["x".to_string()],
        &["result_ptr".to_string(), "llvm_term x".to_string()],
        &PostconditionCtx {
            sub_callee_specs: &[],
            return_type: &TypeInfo::Struct {
                name: "EnrollmentStatus".into(),
                size_bytes: None,
                fields: vec![],
            },
            is_sret: true,
            return_bridge: None,
        },
        &BufferOverrides::default(),
    );

    assert!(
        out.contains("llvm_points_to result_ptr (llvm_term {{ getStatus x }});"),
        "expected sret postcondition to bind into result_ptr; got:\n{out}"
    );
    assert!(
        !out.contains("llvm_return"),
        "sret must NOT use llvm_return (LLVM function returns void); got:\n{out}"
    );
}

/// sret prestate threading: when the Cryptol model has one extra trailing
/// `[N][8]` param, `emit_equiv_spec_body` must emit a `preBytes` fresh
/// variable with an `llvm_points_to` pre-condition on the sret buffer,
/// and the postcondition must include `preBytes` in the Cryptol call.
#[test]
fn emit_sret_prestate_threads_prebytes_into_cryptol_call() {
    let target_spec = SpecConstraint {
        function_name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUEnrollmentStatus@@_N0_NQBUUuid@@@Z".into()),
        params: vec![
            ParamConstraint {
                name: "fE".into(),
                saw_type: "llvm_int 1".into(),
                alloc_type: AllocType::FreshVar,
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            },
            ParamConstraint {
                name: "hK".into(),
                saw_type: "llvm_int 1".into(),
                alloc_type: AllocType::FreshVar,
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            },
            ParamConstraint {
                name: "kA".into(),
                saw_type: "llvm_int 1".into(),
                alloc_type: AllocType::FreshVar,
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            },
            ParamConstraint {
                name: "keyId".into(),
                saw_type: "llvm_array 16 (llvm_int 8)".into(),
                alloc_type: AllocType::AllocReadonly,
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            },
        ],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_array 20 (llvm_int 8)".into(),
            value_constraints: vec![],
            is_sret: true,
            returns_pointer: false,
            sret_prestate: false,
        },
        referenced_globals: vec![],
        has_body: true,
        is_virtual: false,
        can_throw: false,
        postconditions: vec![],
    };
    let target_fn = FunctionInfo {
        name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUEnrollmentStatus@@_N0_NQBUUuid@@@Z".into()),
        return_type: TypeInfo::Struct {
            name: "EnrollmentStatus".into(),
            size_bytes: Some(20),
            fields: vec![],
        },
        params: vec![
            ParamInfo {
                name: "fE".into(),
                ty: TypeInfo::Bool,
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "hK".into(),
                ty: TypeInfo::Bool,
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "kA".into(),
                ty: TypeInfo::Bool,
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "keyId".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::ByteArray(16))),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ],
        called_functions: vec![],
        has_body: true,
        is_virtual: false,
        is_system: false,
        can_throw: false,
        annotations: vec![],
        referenced_globals: vec![],
    };
    let interface_classes = HashSet::new();
    let interface_of = |_: &TypeInfo| -> Option<String> { None };

    let mut out = String::new();
    let (cryptol_args, execute_args) = emit_equiv_spec_body(
        &mut out,
        1,
        "getStatus",
        "getStatus_cpp",
        &target_spec,
        &target_fn,
        &interface_classes,
        &interface_of,
        &[],
        Some(&SretPrestate {
            buf_size: 20,
            take_bytes: 17,
            drop_bytes: 3,
        }),
        &BufferOverrides::default(),
    );

    // preBytes is allocated at FULL buffer size (20), not the slice size (17)
    assert!(
        out.contains("preBytes <- llvm_fresh_var \"preBytes\" (llvm_array 20 (llvm_int 8))"),
        "expected preBytes at full buffer size; got:\n{out}"
    );
    // The pre-condition binding into the sret buffer
    assert!(
        out.contains("llvm_points_to result_ptr (llvm_term preBytes)"),
        "expected preBytes pre-condition on sret buffer; got:\n{out}"
    );
    // The Cryptol arg should be a take/drop slice, NOT the raw preBytes
    let slice_expr = "(take`{17} (drop`{3} preBytes))";
    assert!(
        cryptol_args.contains(&slice_expr.to_string()),
        "expected slice expression in cryptol_args; got: {cryptol_args:?}"
    );
    // result_ptr should be in execute_args
    assert!(
        execute_args.contains(&"result_ptr".to_string()),
        "expected result_ptr in execute_args; got: {execute_args:?}"
    );

    // Now emit the postcondition and verify the slice appears in the
    // Cryptol call
    emit_postcondition_and_close(
        &mut out,
        "getStatus_cpp",
        &cryptol_args,
        &execute_args,
        &PostconditionCtx {
            sub_callee_specs: &[],
            return_type: &TypeInfo::Struct {
                name: "EnrollmentStatus".into(),
                size_bytes: Some(20),
                fields: vec![],
            },
            is_sret: true,
            return_bridge: None,
        },
        &BufferOverrides::default(),
    );

    assert!(
        out.contains(&format!(
            "getStatus_cpp (fE ! 0) (hK ! 0) (kA ! 0) keyId {slice_expr}"
        )),
        "expected slice expression in Cryptol call; got:\n{out}"
    );
}

/// Without sret prestate (None), the emitter should NOT produce preBytes.
#[test]
fn emit_sret_no_prestate_omits_prebytes() {
    let target_spec = SpecConstraint {
        function_name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUEnrollmentStatus@@_N@Z".into()),
        params: vec![ParamConstraint {
            name: "fE".into(),
            saw_type: "llvm_int 1".into(),
            alloc_type: AllocType::FreshVar,
            preconditions: vec![],
            unchanged_after: false,
            dereferenceable_size: None,
        }],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_array 20 (llvm_int 8)".into(),
            value_constraints: vec![],
            is_sret: true,
            returns_pointer: false,
            sret_prestate: false,
        },
        referenced_globals: vec![],
        has_body: true,
        is_virtual: false,
        can_throw: false,
        postconditions: vec![],
    };
    let target_fn = FunctionInfo {
        name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUEnrollmentStatus@@_N@Z".into()),
        return_type: TypeInfo::Struct {
            name: "EnrollmentStatus".into(),
            size_bytes: Some(20),
            fields: vec![],
        },
        params: vec![ParamInfo {
            name: "fE".into(),
            ty: TypeInfo::Bool,
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }],
        called_functions: vec![],
        has_body: true,
        is_virtual: false,
        is_system: false,
        can_throw: false,
        annotations: vec![],
        referenced_globals: vec![],
    };
    let interface_classes = HashSet::new();
    let interface_of = |_: &TypeInfo| -> Option<String> { None };

    let mut out = String::new();
    let (cryptol_args, _) = emit_equiv_spec_body(
        &mut out,
        1,
        "getStatus",
        "getStatus_cpp",
        &target_spec,
        &target_fn,
        &interface_classes,
        &interface_of,
        &[],
        None, // no prestate
        &BufferOverrides::default(),
    );

    assert!(
        !out.contains("preBytes"),
        "should NOT emit preBytes without prestate; got:\n{out}"
    );
    assert!(
        !cryptol_args.contains(&"preBytes".to_string()),
        "preBytes should NOT be in cryptol_args; got: {cryptol_args:?}"
    );
}

// ---------------------------------------------------------------------------
// BEGIN_PROOF / PROVED marker contract (saw-spec-gen-dtb)
// ---------------------------------------------------------------------------
//
// SAW's `llvm_verify` (and `prove_print`) is silent on success. To let
// downstream aggregators turn a single SAW invocation into per-property
// results, the emitter wraps every verification command with stable
// marker lines:
//
//     print "BEGIN_PROOF <function>";
//     <verification command>
//     print "PROVED <function>";
//
// See docs/proof-markers.md for the full contract.

#[test]
fn emit_verify_step_emits_begin_proof_marker_before_llvm_verify() {
    let mut out = String::new();
    let iface = InterfaceCtx {
        has_interfaces: false,
        vmethods: &[],
        constructors: &[],
    };
    emit_verify_step(
        &mut out,
        1,
        "add_one",
        "add_one_spec",
        "add_one",
        &iface,
        vec![],
    );

    assert!(
        out.contains("print \"BEGIN_PROOF add_one\";"),
        "expected BEGIN_PROOF marker; got:\n{out}"
    );
    let begin_idx = out
        .find("print \"BEGIN_PROOF add_one\";")
        .expect("BEGIN_PROOF marker missing");
    let verify_idx = out.find("llvm_verify").expect("llvm_verify missing");
    assert!(
        begin_idx < verify_idx,
        "BEGIN_PROOF must appear before llvm_verify; got:\n{out}"
    );
}

#[test]
fn emit_verify_step_emits_proved_marker_after_llvm_verify() {
    let mut out = String::new();
    let iface = InterfaceCtx {
        has_interfaces: false,
        vmethods: &[],
        constructors: &[],
    };
    emit_verify_step(
        &mut out,
        1,
        "add_one",
        "add_one_spec",
        "add_one",
        &iface,
        vec![],
    );

    assert!(
        out.contains("print \"PROVED add_one\";"),
        "expected PROVED marker; got:\n{out}"
    );
    let verify_idx = out.find("llvm_verify").expect("llvm_verify missing");
    let proved_idx = out
        .find("print \"PROVED add_one\";")
        .expect("PROVED marker missing");
    assert!(
        verify_idx < proved_idx,
        "PROVED must appear after llvm_verify; got:\n{out}"
    );
}

#[test]
fn emit_verify_step_marker_uses_mangled_independent_function_name() {
    // Marker name follows the source `function_name`, not the mangled
    // symbol. This is the stable key Parse-PropertyLog.ps1 keys on.
    let mut out = String::new();
    let iface = InterfaceCtx {
        has_interfaces: false,
        vmethods: &[],
        constructors: &[],
    };
    emit_verify_step(
        &mut out,
        2,
        "compute_hash",
        "compute_hash_cry",
        "?compute_hash@@YAXH@Z",
        &iface,
        vec![],
    );

    assert!(
        out.contains("print \"BEGIN_PROOF compute_hash\";"),
        "got:\n{out}"
    );
    assert!(
        out.contains("print \"PROVED compute_hash\";"),
        "got:\n{out}"
    );
}
