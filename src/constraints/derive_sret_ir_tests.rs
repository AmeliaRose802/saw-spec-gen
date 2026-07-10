//! Tests for `correct_sret_from_ir` – specifically the new branch that
//! promotes `is_sret = false` to `is_sret = true` when the IR first
//! parameter carries the `sret` attribute (issue #86).

use super::*;
use crate::constraints::{Annotation, Mutability, Nullability, ParamInfo};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_func(name: &str, mangled: &str, ret: TypeInfo) -> FunctionInfo {
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

/// Build a `FunctionInfo` that looks like the LLVM IR for a function whose
/// first parameter is `ptr sret(%"class.std::array<unsigned char, 32>")`.
/// This mirrors what `extract_sret` produces after parsing: the sret param
/// is kept in `params[0]` (with `Annotation::Custom("sret")`), and
/// `return_type` is promoted to the inner struct type.
fn ir_func_with_sret(quoted_name: &str) -> FunctionInfo {
    let struct_ty = TypeInfo::Opaque {
        name: "class.std::array<unsigned char, 32>".into(),
        size_bytes: 32,
    };
    let sret_param = ParamInfo {
        name: "sret_result".into(),
        ty: struct_ty.clone(),
        mutability: Mutability::WriteOnly,
        nullable: Nullability::NonNull,
        annotations: vec![Annotation::Custom("sret".into())],
    };
    FunctionInfo {
        name: quoted_name.into(),
        mangled_name: None,
        params: vec![sret_param],
        // After extract_sret, return_type is promoted to the struct type.
        return_type: struct_ty,
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        called_functions: vec![],
        referenced_globals: vec![],
        annotations: vec![],
    }
}

/// IR function with no sret (normal scalar return).
fn ir_func_no_sret(quoted_name: &str, ret: TypeInfo) -> FunctionInfo {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When the AST classifies a typedef'd return as non-sret (opaque name with
/// no `<` or `::`, e.g. `HmacDigest`) but the IR first param has `sret`,
/// `correct_sret_from_ir` should promote `is_sret` to `true`.
#[test]
fn correct_sret_from_ir_detects_sret_when_ast_missed_typedef() {
    let mangled = "?hmac_sha256@crypto@@YA?AVHmacDigest@@V?$span@E@std@@@Z";
    // Opaque with a plain alias name – no `<` or `::`, so AST heuristic
    // leaves is_sret = false.
    //
    // size_bytes: 0 is intentional: if the AST knew the size was > 8 it
    // would classify the return as sret on its own.  The bug scenario is
    // specifically when the typedef name is opaque and the size is unknown
    // (0), so the AST heuristic misses the sret ABI lowering.
    let ast_ret = TypeInfo::Opaque {
        name: "HmacDigest".into(),
        size_bytes: 0,
    };
    let mut specs = derive_constraints(&[simple_func("hmac_sha256", mangled, ast_ret)]).unwrap();
    assert!(
        !specs[0].return_constraint.is_sret,
        "AST heuristic should NOT set sret for plain typedef name"
    );

    let ir_name = format!("\"{mangled}\"");
    let ir_funcs = [ir_func_with_sret(&ir_name)];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        specs[0].return_constraint.is_sret,
        "IR sret param should promote is_sret to true"
    );
    // saw_type should be updated from the IR struct type.
    assert!(
        specs[0].return_constraint.saw_type.contains("32"),
        "saw_type should reflect the IR struct (contains size 32): got '{}'",
        specs[0].return_constraint.saw_type
    );
}

/// When both AST and IR agree there is no sret, the result stays false.
#[test]
fn correct_sret_from_ir_no_sret_in_ir_leaves_false() {
    let mangled = "?getVal@@YAHXZ";
    let mut specs =
        derive_constraints(&[simple_func("getVal", mangled, TypeInfo::SignedInt(32))]).unwrap();
    assert!(!specs[0].return_constraint.is_sret);

    let ir_funcs = [ir_func_no_sret(
        &format!("\"{mangled}\""),
        TypeInfo::SignedInt(32),
    )];
    super::correct_sret_from_ir(&mut specs[0], &ir_funcs);
    assert!(
        !specs[0].return_constraint.is_sret,
        "no sret in IR → should remain false"
    );
}

/// When there is no matching IR function the spec is left unchanged.
#[test]
fn correct_sret_from_ir_no_ir_match_leaves_typedef_false() {
    let mangled = "?doThing@@YAVResult@@XZ";
    // size_bytes: 0 is intentional: this tests that when there is no
    // matching IR function, the spec is left unchanged even for a
    // typedef-aliased return type that the AST could not size.
    let ast_ret = TypeInfo::Opaque {
        name: "Result".into(),
        size_bytes: 0,
    };
    let mut specs = derive_constraints(&[simple_func("doThing", mangled, ast_ret)]).unwrap();
    assert!(!specs[0].return_constraint.is_sret);

    super::correct_sret_from_ir(&mut specs[0], &[]);
    assert!(
        !specs[0].return_constraint.is_sret,
        "no IR match → no change"
    );
}
