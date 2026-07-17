//! Tests for `super::derive_constraints`.
//! Extracted from `derive.rs` to keep that file under the 500-line limit.

use super::*;
use crate::constraints::{EnumVariant, ParamInfo};

#[test]
fn test_derive_readonly_param() {
    let func = FunctionInfo {
        name: "test_fn".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "x".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
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
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].params[0].alloc_type, AllocType::AllocReadonly);
    assert!(specs[0].params[0].unchanged_after);
}

#[test]
fn test_derive_mutable_param() {
    let func = FunctionInfo {
        name: "test_fn".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "buf".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
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
    assert_eq!(specs[0].params[0].alloc_type, AllocType::AllocMutable);
    assert!(!specs[0].params[0].unchanged_after);
}

#[test]
fn test_derive_freshvar_for_scalar() {
    let func = FunctionInfo {
        name: "add".into(),
        mangled_name: None,
        params: vec![
            ParamInfo {
                name: "a".into(),
                ty: TypeInfo::SignedInt(32),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "b".into(),
                ty: TypeInfo::SignedInt(32),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ],
        return_type: TypeInfo::SignedInt(32),
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    assert_eq!(specs[0].params[0].alloc_type, AllocType::FreshVar);
    assert_eq!(specs[0].params[1].alloc_type, AllocType::FreshVar);
}

#[test]
fn test_derive_enum_discriminant_constraint() {
    // Value-passed enum (the AttestKeyOwnership case from MSP). Must
    // be allocated FreshVar (otherwise SAW gets `iN*` where the
    // function declares `iN` and rejects the call) and the
    // precondition must reference the bare parameter name `status`,
    // not `status_disc` — there is no `_disc` variable in scope.
    let func = FunctionInfo {
        name: "check_status".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "status".into(),
            ty: TypeInfo::Enum {
                name: "Status".into(),
                variants: vec![
                    EnumVariant::new("Ok", 0),
                    EnumVariant::new("Err", 1),
                    EnumVariant::new("Pending", 2),
                ],
                discriminant_bits: 8,
            },
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }],
        return_type: TypeInfo::Bool,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    assert_eq!(specs[0].params[0].alloc_type, AllocType::FreshVar);
    let pre = specs[0].params[0].preconditions.join("\n");
    assert!(pre.contains("llvm_precond"), "no llvm_precond: {pre}");
    assert!(
        pre.contains("status <= (2 : [8])"),
        "expected `status <= (2 : [8])`, got: {pre}",
    );
    assert!(!pre.contains("status_disc"), "no `status_disc` var: {pre}");
}

#[test]
fn test_derive_enum_indirect_skips_precondition() {
    // Pointer-to-enum (rare but legal). The discriminant lives in
    // heap memory and the equiv-spec/auto-spec emitters disagree on
    // the variable name, so emit no automatic precondition. Users
    // can bind a length / clamp via an ArrayView annotation surface
    // (Cryptol `[n][T]`, SAL `_In_reads_` / `SAW_BUF`, or a sidecar
    // `.spec.toml`) if they need it.
    let func = FunctionInfo {
        name: "set_status".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "out".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Enum {
                name: "Status".into(),
                variants: vec![EnumVariant::new("Ok", 0), EnumVariant::new("Err", 1)],
                discriminant_bits: 32,
            })),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
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
    assert_eq!(specs[0].params[0].alloc_type, AllocType::AllocMutable);
    assert!(specs[0].params[0]
        .preconditions
        .iter()
        .all(|p| !p.contains("llvm_precond")));
}

#[test]
fn test_derive_nothrow_annotation() {
    let func = FunctionInfo {
        name: "safe_fn".into(),
        mangled_name: None,
        params: vec![],
        return_type: TypeInfo::Void,
        can_throw: true,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![Annotation::NoThrow],
    };
    let specs = derive_constraints(&[func]).unwrap();
    assert!(!specs[0].can_throw);
}

#[test]
fn test_derive_return_enum_constraint() {
    let func = FunctionInfo {
        name: "get_status".into(),
        mangled_name: None,
        params: vec![],
        return_type: TypeInfo::Enum {
            name: "Status".into(),
            variants: vec![EnumVariant::new("A", 0), EnumVariant::new("B", 1)],
            discriminant_bits: 32,
        },
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let specs = derive_constraints(&[func]).unwrap();
    let constraints = specs[0].return_constraint.value_constraints.join("\n");
    assert!(
        constraints.contains("ret <= (1 : [32])"),
        "expected `ret <= (1 : [32])`, got: {constraints}",
    );
    assert!(
        !constraints.contains("ret_disc"),
        "must not reference nonexistent `ret_disc`: {constraints}",
    );
}

#[test]
fn test_derive_postconditions_readonly() {
    let func = FunctionInfo {
        name: "read_only".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "data".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(64))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
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
    assert!(specs[0]
        .postconditions
        .iter()
        .any(|p| p.contains("data_before")));
}

#[test]
fn test_derive_must_inspect_result() {
    let func = FunctionInfo {
        name: "important".into(),
        mangled_name: None,
        params: vec![],
        return_type: TypeInfo::SignedInt(32),
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![Annotation::MustInspectResult],
    };
    let specs = derive_constraints(&[func]).unwrap();
    assert!(specs[0]
        .postconditions
        .iter()
        .any(|p| p.contains("Must_inspect_result")));
}

#[test]
fn void_pointer_param_lowers_to_byte_not_comment() {
    // Bug #11 + Bug #15 regression: a `void*` parameter must (a) not
    // produce `"// void"` in the SAW type slot (parsed as a line
    // comment) and (b) be modelled as a pointer-VALUE (`llvm_int 64`
    // + `FreshVar`), not as a 1-byte backing buffer. The buffer model
    // forces the pointer to be non-null and hands the byte contents
    // (not the address) to the Cryptol callsite — breaking the very
    // case that `isNonNull(void*)` was meant to verify. Unit-level
    // pointee lowering is covered by
    // `pointee_saw_type_rewrites_void_to_byte` in saw_type.rs.
    let func = FunctionInfo {
        name: "do_deallocate".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "p".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Void)),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
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
    let param = &derive_constraints(&[func]).unwrap()[0].params[0];
    assert!(!param.saw_type.contains("//"), "got: {}", param.saw_type);
    assert_eq!(param.saw_type, "llvm_int 64");
    assert_eq!(param.alloc_type, AllocType::FreshVar);
    assert!(!param.unchanged_after);
}

#[test]
fn void_pointer_param_is_pointer_value_not_buffer() {
    // Bug #15: `bool isNonNull(void* p)` reads the address itself.
    // Modeling `p` as a 1-byte buffer-pointee forces `p_ptr` to be
    // non-null (so the null case can never even be explored) and
    // passes the *byte at the address* to the Cryptol spec instead
    // of the address. The fix lowers raw `void*` (no buffer-size
    // annotation) to a `FreshVar` of `llvm_int 64`.
    let func = FunctionInfo {
        name: "isNonNull".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "p".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Void)),
            mutability: Mutability::Readonly,
            nullable: Nullability::Nullable,
            annotations: vec![],
        }],
        return_type: TypeInfo::Bool,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let param = &derive_constraints(&[func]).unwrap()[0].params[0];
    assert_eq!(
        param.alloc_type,
        AllocType::FreshVar,
        "raw void* must be passed by value; got {:?}",
        param.alloc_type,
    );
    assert_eq!(param.saw_type, "llvm_int 64");
}

#[test]
fn void_pointer_with_in_reads_annotation_keeps_buffer_model() {
    // Counter-case: when the caller-side contract documents a buffer
    // length via `_In_reads_(N)` (typical for `memcpy(void* dst,
    // const void* src, size_t n)`), the void* IS a buffer and the
    // existing `llvm_array N (llvm_int 8)` model is correct. The
    // Bug #15 fix must not regress this case.
    let func = FunctionInfo {
        name: "process_buffer".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "buf".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Void)),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![Annotation::InReads(16)],
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
    let param = &derive_constraints(&[func]).unwrap()[0].params[0];
    assert_eq!(param.saw_type, "llvm_array 16 (llvm_int 8)");
    assert_eq!(param.alloc_type, AllocType::AllocReadonly);
}

#[test]
fn typed_pointer_param_is_unaffected_by_void_ptr_fix() {
    // Guard: the Bug #15 fix only applies to `Pointer(Void)`. A
    // typed pointer like `int*` keeps the existing buffer model
    // (1-element allocation, points-to fresh value, _ptr execute
    // arg) — flipping it to pointer-value would break the common
    // input/output-parameter case.
    let func = FunctionInfo {
        name: "read_int".into(),
        mangled_name: None,
        params: vec![ParamInfo {
            name: "src".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }],
        return_type: TypeInfo::SignedInt(32),
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    };
    let param = &derive_constraints(&[func]).unwrap()[0].params[0];
    assert_eq!(param.saw_type, "llvm_int 32");
    assert_eq!(param.alloc_type, AllocType::AllocReadonly);
    assert!(param.unchanged_after);
}
