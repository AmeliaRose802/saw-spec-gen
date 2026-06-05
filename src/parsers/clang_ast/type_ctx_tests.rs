use super::*;
use serde_json::json;

fn parse(v: serde_json::Value) -> AstNode {
    serde_json::from_value(v).unwrap()
}

#[test]
fn collects_struct_fields_and_polymorphism() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "CXXRecordDecl",
            "name": "Foo",
            "inner": [
                {"kind": "FieldDecl", "name": "x", "type": {"qualType": "int"}},
                {"kind": "CXXMethodDecl", "name": "f", "virtual": true}
            ]
        }]
    }));
    let ctx = build(&ast);
    assert!(ctx.structs.contains_key("Foo"));
    assert_eq!(ctx.structs["Foo"][0].0, "x");
    assert!(ctx.polymorphic_classes.contains("Foo"));
}

#[test]
fn enums_with_underlying_type_are_recorded() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "EnumDecl",
            "name": "E",
            "fixedUnderlyingType": {"qualType": "uint8_t"},
            "inner": [
                {"kind": "EnumConstantDecl", "name": "A"},
                {"kind": "EnumConstantDecl", "name": "B"}
            ]
        }]
    }));
    let ctx = build(&ast);
    assert_eq!(ctx.enums["E"].1, 8);
    assert_eq!(
        ctx.enums["E"].0,
        vec![EnumVariant::new("A", 0), EnumVariant::new("B", 1)]
    );
}

/// Regression for the gapped-enum bug: when source spells out an
/// explicit `= N` initializer on an `EnumConstantDecl`, the
/// extractor must capture `N` so the constraint emitter can
/// produce a membership disjunction instead of `<= len-1`.
#[test]
fn enum_constants_with_explicit_initializers_capture_value() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "EnumDecl",
            "name": "Status",
            "fixedUnderlyingType": {"qualType": "uint8_t"},
            "inner": [
                {
                    "kind": "EnumConstantDecl", "name": "Ok",
                    "inner": [{
                        "kind": "ConstantExpr",
                        "inner": [{"kind": "IntegerLiteral", "value": "0"}]
                    }]
                },
                {
                    "kind": "EnumConstantDecl", "name": "NotFound",
                    "inner": [{
                        "kind": "ConstantExpr",
                        "inner": [{"kind": "IntegerLiteral", "value": "2"}]
                    }]
                },
                {
                    "kind": "EnumConstantDecl", "name": "Denied",
                    "inner": [{
                        "kind": "ConstantExpr",
                        "inner": [{"kind": "IntegerLiteral", "value": "100"}]
                    }]
                }
            ]
        }]
    }));
    let ctx = build(&ast);
    assert_eq!(
        ctx.enums["Status"].0,
        vec![
            EnumVariant::new("Ok", 0),
            EnumVariant::new("NotFound", 2),
            EnumVariant::new("Denied", 100),
        ]
    );
}

/// When an `EnumConstantDecl` has no initializer, its value is
/// "previous variant + 1, starting at 0" — same as C / Rust.
#[test]
fn enum_constants_without_initializers_continue_running_count() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "EnumDecl",
            "name": "Mix",
            "fixedUnderlyingType": {"qualType": "uint8_t"},
            "inner": [
                {"kind": "EnumConstantDecl", "name": "A"},
                {
                    "kind": "EnumConstantDecl", "name": "B",
                    "inner": [{
                        "kind": "ConstantExpr",
                        "inner": [{"kind": "IntegerLiteral", "value": "10"}]
                    }]
                },
                {"kind": "EnumConstantDecl", "name": "C"}
            ]
        }]
    }));
    let ctx = build(&ast);
    assert_eq!(
        ctx.enums["Mix"].0,
        vec![
            EnumVariant::new("A", 0),
            EnumVariant::new("B", 10),
            EnumVariant::new("C", 11),
        ]
    );
}

/// Regression for the `std::`-qualified underlying-type spelling.
/// Before the fix this fell through to the 32-bit default and the
/// generator emitted `llvm_int 32` for an IR `i8` parameter.
#[test]
fn enums_with_std_qualified_underlying_type_get_correct_width() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "EnumDecl", "name": "AuthResult",
            "fixedUnderlyingType": {"qualType": "std::uint8_t"},
            "inner": [{"kind": "EnumConstantDecl", "name": "Ok"}]
        }]
    }));
    let ctx = build(&ast);
    assert_eq!(ctx.enums["AuthResult"].1, 8);
}

#[test]
fn forward_declared_enum_seeds_empty_variants() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "EnumDecl",
            "name": "Fwd",
            "fixedUnderlyingType": {"qualType": "uint64_t"}
        }]
    }));
    let ctx = build(&ast);
    assert_eq!(ctx.enums["Fwd"].1, 64);
    assert!(ctx.enums["Fwd"].0.is_empty());
}

#[test]
fn mutable_field_propagates_through_bases() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "CXXRecordDecl",
                "name": "Base",
                "inner": [{
                    "kind": "FieldDecl",
                    "name": "m",
                    "type": {"qualType": "int"},
                    "mutable": true
                }]
            },
            {
                "kind": "CXXRecordDecl",
                "name": "Derived",
                "bases": [{"type": {"qualType": "Base"}}],
                "inner": []
            }
        ]
    }));
    let ctx = build(&ast);
    assert!(ctx.classes_with_mutable_field.contains("Base"));
    assert!(ctx.classes_with_mutable_field.contains("Derived"));
}

#[test]
fn compute_class_layout_reuses_topmost_ancestor() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "CXXRecordDecl",
                "name": "OkLog",
                "inner": [{
                    "kind": "FieldDecl",
                    "name": "state",
                    "type": {"qualType": "int"}
                }]
            },
            {
                "kind": "CXXRecordDecl",
                "name": "OkLogV2",
                "bases": [{"type": {"qualType": "OkLog"}}],
                "inner": []
            }
        ]
    }));
    let ctx = build(&ast);
    let (name, fields) = compute_class_layout("OkLogV2", &ctx).unwrap();
    assert_eq!(name, "class.OkLog");
    assert_eq!(fields.len(), 1);
}

#[test]
fn field_default_literal_extracts_int() {
    let f = parse(json!({
        "kind": "FieldDecl",
        "name": "x",
        "hasInClassInitializer": true,
        "type": {"qualType": "int"},
        "inner": [{"kind": "IntegerLiteral", "value": "42"}]
    }));
    assert_eq!(extract_field_default_literal(&f), "42");
}

#[test]
fn field_default_literal_extracts_bool() {
    let f = parse(json!({
        "kind": "FieldDecl",
        "name": "b",
        "hasInClassInitializer": true,
        "type": {"qualType": "bool"},
        "inner": [{"kind": "CXXBoolLiteralExpr", "value": true}]
    }));
    assert_eq!(extract_field_default_literal(&f), "1");
}

#[test]
fn node_id_map_records_only_records() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {"kind": "CXXRecordDecl", "id": "0xa", "name": "A"},
            {"kind": "FunctionDecl", "id": "0xb", "name": "B"}
        ]
    }));
    let map = build_node_id_map(&ast);
    assert_eq!(map.get("0xa").map(String::as_str), Some("A"));
    assert!(!map.contains_key("0xb"));
}
