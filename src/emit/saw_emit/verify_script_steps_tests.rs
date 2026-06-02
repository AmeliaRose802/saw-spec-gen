//! Tests for [`super::verify_script_steps`].
//!
//! Kept in a sibling file so that `verify_script_steps.rs` stays under
//! the 500-non-whitespace-line repository limit.

use super::*;

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
        &[],
        &TypeInfo::Bool,
        false,
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
        &[],
        &TypeInfo::SignedInt(32),
        false,
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
        &[],
        // The C++-level return type is the aggregate; the LLVM-level
        // signature lowers it to void+sret. The is_sret=true flag is
        // what tells the emitter which form to use.
        &TypeInfo::Struct {
            name: "EnrollmentStatus".into(),
            size_bytes: None,
            fields: vec![],
        },
        true,
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

/// When `sret_prestate` is true, the emitter must:
/// 1. Allocate a `result_pre` symbolic var for the sret buffer
/// 2. Bind `result_pre` to `result_ptr` before `llvm_execute_func`
/// 3. Include `result_pre` as a trailing Cryptol argument
#[test]
fn emit_equiv_body_threads_sret_prestate() {
    let target_spec = SpecConstraint {
        function_name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUStatus@@_N0@Z".into()),
        params: vec![
            ParamConstraint {
                name: "a".into(),
                alloc_type: AllocType::FreshVar,
                saw_type: "llvm_int 1".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            },
            ParamConstraint {
                name: "b".into(),
                alloc_type: AllocType::FreshVar,
                saw_type: "llvm_int 1".into(),
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
            sret_prestate: true,
        },
        can_throw: false,
        is_virtual: false,
        has_body: true,
        referenced_globals: vec![],
        postconditions: vec![],
    };
    let target_fn = FunctionInfo {
        name: "getStatus".into(),
        mangled_name: Some("?getStatus@@YA?AUStatus@@_N0@Z".into()),
        params: vec![
            ParamInfo {
                name: "a".into(),
                ty: TypeInfo::Bool,
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "b".into(),
                ty: TypeInfo::Bool,
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ],
        return_type: TypeInfo::Struct {
            name: "Status".into(),
            size_bytes: Some(20),
            fields: vec![],
        },
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        annotations: vec![],
        referenced_globals: vec![],
        called_functions: vec![],
    };
    let iface_classes = std::collections::HashSet::new();
    let iface_of = |_: &TypeInfo| -> Option<String> { None };

    let mut out = String::new();
    let (cryptol_args, execute_args) = emit_equiv_spec_body(
        &mut out,
        1,
        "getStatus",
        "getStatus",
        &target_spec,
        &target_fn,
        &iface_classes,
        &iface_of,
        &[],
    );

    // result_pre must appear as a fresh var + points_to before execute_func
    assert!(
        out.contains("result_pre <- llvm_fresh_var"),
        "expected result_pre allocation; got:\n{out}"
    );
    assert!(
        out.contains("llvm_points_to result_ptr (llvm_term result_pre)"),
        "expected pre-state binding; got:\n{out}"
    );
    // result_pre must be the trailing Cryptol argument
    assert_eq!(
        cryptol_args.last().map(|s| s.as_str()),
        Some("result_pre"),
        "result_pre must be trailing Cryptol arg; got: {cryptol_args:?}"
    );
    // result_ptr must be in execute_args (sret pointer)
    assert!(
        execute_args.contains(&"result_ptr".to_string()),
        "execute_args must include result_ptr; got: {execute_args:?}"
    );

    // Now verify the postcondition includes result_pre in the Cryptol call
    emit_postcondition_and_close(
        &mut out,
        "getStatus",
        &cryptol_args,
        &execute_args,
        &[],
        &TypeInfo::Struct {
            name: "Status".into(),
            size_bytes: Some(20),
            fields: vec![],
        },
        true,
    );
    assert!(
        out.contains("getStatus (a ! 0) (b ! 0) result_pre"),
        "Cryptol call must include result_pre trailing arg; got:\n{out}"
    );
}
