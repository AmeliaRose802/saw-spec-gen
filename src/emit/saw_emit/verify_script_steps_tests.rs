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
