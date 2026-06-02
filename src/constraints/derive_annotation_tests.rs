//! Tests for SAL-annotation handling inside `super::derive_constraints`.
//! Split out of `derive_tests.rs` to keep individual test files under
//! the 500-non-whitespace-line repository limit.

use super::*;
use crate::constraints::ParamInfo;

#[test]
fn test_derive_nullable_param_comment() {
    let func = FunctionInfo {
        name: "maybe_null".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "ptr".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            mutability: Mutability::Readonly,
            nullable: Nullability::Nullable,
            annotations: vec![],
        }],
        return_type: TypeInfo::Void,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    // Unsized pointer (no _In_reads_/_Out_writes_ annotation) now
    // emits a loud TODO that names the parameter and tells the user to
    // either add a SAL annotation or hand-edit the allocation. The
    // older "may be null -- spec assumes non-null" wording was
    // dropped because it conflated nullability with buffer-size guess.
    let preconds = &specs[0].params[0].preconditions;
    assert!(
        preconds.iter().any(|p| p.contains("TODO[saw-spec-gen]")),
        "expected loud TODO for unsized pointer; got {preconds:?}",
    );
    assert!(
        preconds.iter().any(|p| p.contains("`ptr`")),
        "expected TODO to name the parameter; got {preconds:?}",
    );
}

#[test]
fn test_derive_unsized_pointer_detects_length_companion() {
    // Two-param fn `f(buf, n)` — the unsized `buf` should pick up
    // `n` as the heuristic length companion.
    let func = FunctionInfo {
        name: "copy_n".into(),
        mangled_name: None,
        params: vec![
            ParamInfo {
                name: "buf".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "n".into(),
                ty: TypeInfo::UnsignedInt(8),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ],
        return_type: TypeInfo::Void,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    let buf_preconds = &specs[0].params[0].preconditions;
    assert!(
        buf_preconds
            .iter()
            .any(|p| p.contains("sibling parameter `n`")),
        "expected length-companion heuristic to flag `n`; got {buf_preconds:?}",
    );
}

#[test]
fn test_derive_paramref_in_reads_emits_precondition() {
    // _In_reads_(n) (paramref SAL) should:
    //   * size the buffer to DEFAULT_PARAMREF_MAX_LEN
    //   * emit a TODO comment explaining the upper-bound guess
    //   * emit a real llvm_precond binding `n` <= MAX_LEN
    use super::DEFAULT_PARAMREF_MAX_LEN;
    let func = FunctionInfo {
        name: "f".into(),
        mangled_name: None,
        params: vec![
            ParamInfo {
                name: "src".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![Annotation::InReadsParam("n".into())],
            },
            ParamInfo {
                name: "n".into(),
                ty: TypeInfo::UnsignedInt(8),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ],
        return_type: TypeInfo::Void,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    let src_param = &specs[0].params[0];
    assert_eq!(
        src_param.saw_type,
        format!("llvm_array {DEFAULT_PARAMREF_MAX_LEN} (llvm_int 8)"),
        "paramref InReads should size buffer to DEFAULT_PARAMREF_MAX_LEN",
    );
    assert!(
        src_param
            .preconditions
            .iter()
            .any(|p| p.contains("llvm_precond") && p.contains("n") && p.contains("<=")),
        "expected llvm_precond bounding `n`; got {:?}",
        src_param.preconditions,
    );
    assert!(
        src_param
            .preconditions
            .iter()
            .any(|p| p.contains("TODO[saw-spec-gen]")),
        "expected TODO marker; got {:?}",
        src_param.preconditions,
    );
}
