use super::*;
use crate::buffer_overrides::BufferOverrides;
use std::collections::HashSet;

#[test]
fn emit_equiv_spec_body_supports_auto_out_buffer_for_this() {
    let target_spec = SpecConstraint {
        function_name: "activate".into(),
        mangled_name: Some("?activate@KeyStore@@QEAAHXZ".into()),
        params: vec![ParamConstraint {
            name: "this".into(),
            saw_type: "llvm_alias \"class.sdep::KeyStore\"".into(),
            alloc_type: AllocType::AllocMutable,
            preconditions: vec![],
            unchanged_after: false,
            dereferenceable_size: None,
            out_postcond: None,
        }],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_int 32".into(),
            value_constraints: vec![],
            is_sret: false,
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
        name: "activate".into(),
        mangled_name: Some("?activate@KeyStore@@QEAAHXZ".into()),
        return_type: TypeInfo::SignedInt(32),
        params: vec![ParamInfo {
            name: "this".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "class.sdep::KeyStore".into(),
                size_bytes: 16,
            })),
            mutability: Mutability::Mutable,
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
    let overrides = BufferOverrides::from_cli(
        &[],
        &["this=auto".into()],
        &["this=activate_post".into()],
        &[],
        &[],
        &[],
    )
    .unwrap();

    let mut out = String::new();
    let (cryptol_args, execute_args) = emit_equiv_spec_body(
        &mut out,
        1,
        "activate",
        "activate_ret",
        &target_spec,
        &target_fn,
        &interface_classes,
        &interface_of,
        &[],
        None,
        &overrides,
    );

    assert!(
        out.contains("this_ptr <- llvm_alloc (llvm_alias \"class.sdep::KeyStore\")"),
        "expected mutable allocation for auto out-buffer `this`; got:\n{out}"
    );
    assert!(
        out.contains(
            "this_pre <- llvm_fresh_var \"this_pre\" (llvm_alias \"class.sdep::KeyStore\")"
        ),
        "expected pre-state var for `this`; got:\n{out}"
    );
    assert!(
        cryptol_args.contains(&"this_pre".to_string()),
        "expected cryptol args to use `this_pre`; got: {cryptol_args:?}"
    );
    assert!(
        execute_args.contains(&"this_ptr".to_string()),
        "expected execute args to pass `this_ptr`; got: {execute_args:?}"
    );
}

#[test]
fn emit_equiv_spec_body_emits_cryptol_precondition_call() {
    let target_spec = SpecConstraint {
        function_name: "activate".into(),
        mangled_name: Some("?activate@KeyStore@@QEAAHXZ".into()),
        params: vec![ParamConstraint {
            name: "x".into(),
            saw_type: "llvm_int 32".into(),
            alloc_type: AllocType::FreshVar,
            preconditions: vec![],
            unchanged_after: false,
            dereferenceable_size: None,
            out_postcond: None,
        }],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_int 32".into(),
            value_constraints: vec![],
            is_sret: false,
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
        name: "activate".into(),
        mangled_name: Some("?activate@KeyStore@@QEAAHXZ".into()),
        return_type: TypeInfo::SignedInt(32),
        params: vec![ParamInfo {
            name: "x".into(),
            ty: TypeInfo::SignedInt(32),
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
    let overrides = BufferOverrides::from_cli(
        &[],
        &[],
        &[],
        &[],
        &["activate_pre=x".into()],
        &["activate_pre".into()],
    )
    .unwrap();

    let mut out = String::new();
    let _ = emit_equiv_spec_body(
        &mut out,
        1,
        "activate",
        "activate_ret",
        &target_spec,
        &target_fn,
        &interface_classes,
        &interface_of,
        &[],
        None,
        &overrides,
    );

    assert!(
        out.contains("llvm_precond {{ activate_pre x }};"),
        "expected Cryptol precondition call; got:\n{out}"
    );
}
