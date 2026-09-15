//! Tests for `super::correct_sret_from_ir` (both the register-return
//! clearing path and the sret-recovery path). Extracted from
//! `derive_tests.rs` to keep that file under the 500-line limit.

use super::*;
use crate::constraints::ParamInfo;

fn make_func(name: &str, mangled: &str, ret: TypeInfo) -> FunctionInfo {
    FunctionInfo {
        name: name.into(),
        mangled_name: Some(mangled.into()),
        params: vec![],
        return_type: ret,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    }
}

fn make_ir_func(quoted_name: &str, ret: TypeInfo) -> FunctionInfo {
    FunctionInfo {
        name: quoted_name.into(),
        mangled_name: None,
        params: vec![],
        return_type: ret,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    }
}

// ---- correct_sret_from_ir tests ----------------------------------------

#[test]
fn correct_sret_from_ir_resolves_scalar_typedef_returns() {
    let mangled = "?size@Container@@QEBA_KXZ";
    for (ir_type, saw_type) in [
        ("i1", "llvm_int 1"),
        ("i8", "llvm_int 8"),
        ("i16", "llvm_int 16"),
        ("i32", "llvm_int 32"),
        ("i64", "llvm_int 64"),
        ("i128", "llvm_int 128"),
    ] {
        // An unresolved alias or its byte-buffer fallback is not an ABI type.
        for size_bytes in [0, 1] {
            let ast_ret = TypeInfo::Opaque {
                name: "size_type".into(),
                size_bytes,
            };
            let mut specs = derive_constraints(&[make_func("size", mangled, ast_ret)]).unwrap();
            assert!(!specs[0].return_constraint.is_sret);
            let ir_funcs = crate::llvm_ir::extract_functions(
                &format!("declare noundef {ir_type} @\"{mangled}\"()"),
                None,
            )
            .unwrap();

            super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
            assert_eq!(specs[0].return_constraint.saw_type, saw_type);
            assert!(!specs[0].return_constraint.is_sret);
            assert!(!specs[0].return_constraint.returns_pointer);
        }
    }
}

#[test]
fn correct_sret_from_ir_preserves_pointer_pointee_type() {
    let mangled = "?data@Container@@QEBAPEBIXZ";
    let ast_ret = TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(32)));
    let mut specs = derive_constraints(&[make_func("data", mangled, ast_ret)]).unwrap();
    let ir_funcs =
        crate::llvm_ir::extract_functions(&format!("declare ptr @\"{mangled}\"()"), None).unwrap();

    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(specs[0].return_constraint.returns_pointer);
    assert!(!specs[0].return_constraint.is_sret);
    assert_eq!(specs[0].return_constraint.saw_type, "llvm_int 32");
}

#[test]
fn correct_sret_from_ir_preserves_sret_with_scalar_pointee() {
    let mangled = "?result@@YA?AUResult@@XZ";
    let ast_ret = TypeInfo::Struct {
        name: "Result".into(),
        size_bytes: Some(8),
        fields: vec![],
    };
    let mut specs = derive_constraints(&[make_func("result", mangled, ast_ret)]).unwrap();
    let ir_funcs = crate::llvm_ir::extract_functions(
        &format!("declare void @\"{mangled}\"(ptr sret(i64) %result)"),
        None,
    )
    .unwrap();
    assert_eq!(ir_funcs[0].return_type, TypeInfo::SignedInt(64));

    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(specs[0].return_constraint.is_sret);
    assert_eq!(
        specs[0].return_constraint.saw_type,
        "llvm_array 8 (llvm_int 8)"
    );
}

#[test]
fn correct_sret_from_ir_overrides_small_struct_register_return() {
    let mangled = "?enforceAccess@@YA?AUOutcome@@W4Mode@@@Z";
    let ast_ret = TypeInfo::Struct {
        name: "Outcome".into(),
        size_bytes: Some(2),
        fields: vec![],
    };
    let mut specs = derive_constraints(&[make_func("enforceAccess", mangled, ast_ret)]).unwrap();
    assert!(
        specs[0].return_constraint.is_sret,
        "AST heuristic should set sret"
    );

    let ir_funcs = [make_ir_func(
        &format!("\"{mangled}\""),
        TypeInfo::SignedInt(16),
    )];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        !specs[0].return_constraint.is_sret,
        "IR override should clear sret"
    );
    assert_eq!(specs[0].return_constraint.saw_type, "llvm_int 16");
}

#[test]
fn correct_sret_from_ir_keeps_genuine_sret() {
    let mangled = "?getLargeResult@@YA?AUBig@@XZ";
    let struct_ty = TypeInfo::Struct {
        name: "Big".into(),
        size_bytes: Some(128),
        fields: vec![],
    };
    let mut specs =
        derive_constraints(&[make_func("getLargeResult", mangled, struct_ty.clone())]).unwrap();
    assert!(specs[0].return_constraint.is_sret);

    let ir_funcs = [make_ir_func(&format!("\"{mangled}\""), struct_ty)];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        specs[0].return_constraint.is_sret,
        "genuine sret should remain"
    );
}

#[test]
fn correct_sret_from_ir_no_match_leaves_unchanged() {
    let ast_ret = TypeInfo::Struct {
        name: "S".into(),
        size_bytes: Some(4),
        fields: vec![],
    };
    let mut specs = derive_constraints(&[make_func("f", "?f@@YAHXZ", ast_ret)]).unwrap();
    assert!(specs[0].return_constraint.is_sret);

    super::correct_sret_from_ir(&mut specs[0], &[]);
    assert!(specs[0].return_constraint.is_sret, "no IR → no change");
}

// ---- sret recovery tests -----------------------------------------------

fn make_ir_func_with_sret(quoted_name: &str, effective_ret: TypeInfo) -> FunctionInfo {
    FunctionInfo {
        name: quoted_name.into(),
        mangled_name: None,
        // extract_sret places the sret pointer at params[0] and keeps
        // its Custom("sret") marker; the return type is promoted to the
        // inner aggregate.
        params: vec![ParamInfo {
            name: "result".into(),
            ty: effective_ret.clone(),
            mutability: Mutability::WriteOnly,
            nullable: Nullability::NonNull,
            annotations: vec![Annotation::Custom("sret".into())],
        }],
        return_type: effective_ret,
        can_throw: false,
        is_virtual: false,
        has_body: false,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    }
}

#[test]
fn ensure_sret_from_ir_recovers_forward_declared_aggregate() {
    // A forward-declared digest type returned by value parses as an
    // unsized Opaque with a plain (non-namespaced) name, so the AST
    // heuristic leaves is_sret = false.
    let mangled = "?hmac_sha256@@YA?AUHmac@@XZ";
    let ast_ret = TypeInfo::Opaque {
        name: "Hmac".into(),
        size_bytes: 0,
    };
    let mut specs = derive_constraints(&[make_func("hmac_sha256", mangled, ast_ret)]).unwrap();
    assert!(
        !specs[0].return_constraint.is_sret,
        "AST heuristic misses plain-name opaque aggregate"
    );

    let ir_funcs = [make_ir_func_with_sret(
        &format!("\"{mangled}\""),
        TypeInfo::ByteArray(32),
    )];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        specs[0].return_constraint.is_sret,
        "IR sret param should recover sret classification"
    );
    assert_eq!(
        specs[0].return_constraint.saw_type, "llvm_array 32 (llvm_int 8)",
        "buffer sized from IR effective return type"
    );
}

#[test]
fn ensure_sret_from_ir_leaves_register_return_untouched() {
    // No sret param in the IR → nothing to recover.
    let mangled = "?smallReturn@@YAHXZ";
    let ast_ret = TypeInfo::Opaque {
        name: "Tiny".into(),
        size_bytes: 0,
    };
    let mut specs = derive_constraints(&[make_func("smallReturn", mangled, ast_ret)]).unwrap();
    assert!(!specs[0].return_constraint.is_sret);

    let ir_funcs = [make_ir_func(
        &format!("\"{mangled}\""),
        TypeInfo::SignedInt(32),
    )];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        !specs[0].return_constraint.is_sret,
        "no IR sret param → no change"
    );
}

#[test]
fn ensure_sret_from_ir_no_ir_leaves_unchanged() {
    let mangled = "?hmac_sha256@@YA?AUHmac@@XZ";
    let ast_ret = TypeInfo::Opaque {
        name: "Hmac".into(),
        size_bytes: 0,
    };
    let mut specs = derive_constraints(&[make_func("hmac_sha256", mangled, ast_ret)]).unwrap();
    assert!(!specs[0].return_constraint.is_sret);

    super::correct_sret_from_ir(&mut specs[0], &[]);
    assert!(!specs[0].return_constraint.is_sret, "no IR → no change");
}
