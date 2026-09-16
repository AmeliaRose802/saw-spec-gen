use super::*;
use serde_json::json;

fn ast(value: Value) -> AstNode {
    serde_json::from_value(value).unwrap()
}

fn declaration(name: &str, values: &[Option<&str>]) -> Value {
    let constants: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let inner: Vec<_> = value
                .iter()
                .map(|value| json!({"kind": "ConstantExpr", "value": value}))
                .collect();
            json!({"kind": "EnumConstantDecl", "name": format!("v{i}"), "inner": inner})
        })
        .collect();
    json!({"kind": "EnumDecl", "name": name, "inner": constants})
}

fn field(source: &str, bits: usize) -> FieldLayout {
    FieldLayout {
        path: "value".into(),
        source_type: source.into(),
        llvm_type: format!("i{bits}"),
        offset: 0,
        size: bits.div_ceil(8),
        bit_offset: None,
        bit_width: None,
        array_count: None,
        array_stride: None,
        guard: None,
        validity: None,
        is_pointer: false,
        runtime: false,
    }
}

fn layout(fields: Vec<FieldLayout>) -> ObjectLayout {
    ObjectLayout {
        source_type: "struct Holder".into(),
        llvm_type: "struct.Holder".into(),
        allocation_type: "llvm_alias \"struct.Holder\"".into(),
        size: fields.iter().map(|f| f.offset + f.size).max().unwrap_or(0),
        alignment: 1,
        llvm_alignment: 1,
        fields,
        padding: Vec::new(),
        bases: Vec::new(),
        validation: Vec::new(),
        unresolved: Vec::new(),
    }
}

fn applied(source: &str, bits: usize, declaration: Value) -> ObjectLayout {
    let mut object = layout(vec![field(source, bits)]);
    apply(&mut object, &ast(declaration)).unwrap();
    object
}

fn rejects(source: &str, bits: usize, declaration: Value) -> String {
    let mut object = layout(vec![field(source, bits)]);
    let error = apply(&mut object, &ast(declaration)).unwrap_err();
    assert!(object.fields[0].validity.is_none());
    assert!(object
        .validation
        .iter()
        .any(|v| v.contains("unresolved enum validity")));
    format!("{error:#}")
}

fn tag(object: &ObjectLayout) -> Option<&str> {
    object.fields[0].validity.as_deref()
}

#[test]
fn fixed_sparse_scoped_and_opaque_enums_have_no_restriction() {
    for metadata in [
        json!({"fixedUnderlyingType": {"qualType": "unsigned char"}}),
        json!({"scopedEnumTag": "class"}),
        json!({"scopedEnumTag": "struct"}),
    ] {
        let mut decl = declaration("E", &[Some("0"), Some("17")]);
        decl.as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        let mut object = layout(vec![field("enum E", 8)]);
        object.fields[0].validity = Some("enum_unsigned:31".into());
        apply(&mut object, &ast(decl.clone())).unwrap();
        assert!(object.fields[0].validity.is_none());
        assert!(predicate(&object.fields[0], "x").is_none());
        assert!(object
            .validation
            .iter()
            .any(|v| v.contains("all underlying values valid")));
        decl["inner"] = json!([]); // Opaque fixed declaration needs no enumerators.
        apply(&mut object, &ast(decl)).unwrap();
    }
}

#[test]
fn nonfixed_sparse_values_allow_gaps_and_normalize_elaborated_names() {
    let decl = json!({"kind": "NamespaceDecl", "name": "ns", "inner": [
        declaration("E", &[Some("0"), Some("4")])
    ]});
    for source in [
        "E",
        "enum E",
        "ns::E",
        "enum ns::E",
        "enum class ns::E",
        "const enum ns::E volatile",
        "::ns::E",
        "enum ::ns::E",
    ] {
        let object = applied(source, 32, decl.clone());
        let field = &object.fields[0];
        assert_eq!(field.validity.as_deref(), Some("enum_unsigned:7"));
        assert_eq!(predicate(field, "x").unwrap(), "(x <= (7 : [32]))");
        // The bound includes 1, 2, 3 and 5, 6, 7, not just enumerators 0 and 4.
        let (_, max) = tag(&object).unwrap().split_once(':').unwrap();
        let max: u32 = max.parse().unwrap();
        assert!((0..=7).all(|value| value <= max));
    }
}

#[test]
fn signed_ranges_cover_both_extrema_with_signed_comparisons() {
    for (values, expected) in [
        (vec![Some("-3"), Some("5")], "enum_signed:-8:7"),
        (vec![Some("-8"), Some("0")], "enum_signed:-8:7"),
        (vec![Some("-9"), Some("1")], "enum_signed:-16:15"),
        (vec![Some("-1")], "enum_signed:-1:0"),
        (vec![Some("-2"), Some("-1")], "enum_signed:-2:1"),
        (vec![Some("-1"), Some("8")], "enum_signed:-16:15"),
    ] {
        let object = applied("E", 32, declaration("E", &values));
        assert_eq!(object.fields[0].validity.as_deref(), Some(expected));
    }
    let object = applied("E", 32, declaration("E", &[Some("-3"), Some("5")]));
    assert_eq!(
        predicate(&object.fields[0], "x").unwrap(),
        "(x <=$ (7 : [32]) && x >=$ (-8 : [32]))"
    );
}

#[test]
fn implicit_sequence_uses_previous_value_not_the_largest_enumerator() {
    for (values, expected) in [
        (
            vec![None, None, Some("7"), Some("0"), None],
            "enum_unsigned:7",
        ),
        (vec![Some("7"), None], "enum_unsigned:15"),
        (vec![Some("-1"), None, None], "enum_signed:-2:1"),
    ] {
        let object = applied("E", 32, declaration("E", &values));
        assert_eq!(tag(&object), Some(expected));
    }
}

#[test]
fn evaluated_constants_override_alias_and_operator_subexpressions() {
    let decl = json!({"kind": "EnumDecl", "name": "E", "inner": [
        {"kind": "EnumConstantDecl", "name": "negative", "inner": [
            {"kind": "ConstantExpr", "value": "-3", "inner": [
                {"kind": "IntegerLiteral", "value": "999"}
            ]}
        ]},
        {"kind": "EnumConstantDecl", "name": "alias", "inner": [
            {"kind": "ImplicitCastExpr", "castKind": "IntegralCast", "inner": [
                {"kind": "ConstantExpr", "value": "5", "inner": [
                    {"kind": "DeclRefExpr", "referencedDecl": {"name": "other"}}
                ]}
            ]}
        ]}
    ]});
    assert_eq!(tag(&applied("E", 32, decl)), Some("enum_signed:-8:7"));
}

#[test]
fn literals_unary_values_parentheses_and_attributes_are_supported() {
    let decl = json!({"kind": "EnumDecl", "name": "E", "inner": [
        {"kind": "EnumConstantDecl", "name": "a", "inner": [
            {"kind": "UnaryOperator", "opcode": "-", "type": {"qualType": "int"},
             "inner": [{"kind": "ParenExpr", "inner": [{"kind": "IntegerLiteral", "value": 3}]}]}
        ]},
        {"kind": "EnumConstantDecl", "name": "b", "inner": [{"kind": "DeprecatedAttr"}]},
        {"kind": "EnumConstantDecl", "name": "c", "inner": [
            {"kind": "UnaryOperator", "opcode": "+", "value": "5"}
        ]},
        {"kind": "EnumConstantDecl", "name": "d", "inner": [{"kind": "IntegerLiteral", "value": "6"}]}
    ]});
    assert_eq!(tag(&applied("E", 32, decl)), Some("enum_signed:-8:7"));
}

#[test]
fn empty_and_zero_only_enums_have_a_one_bit_range() {
    for values in [vec![], vec![Some("0")], vec![Some("-0"), Some("0")]] {
        let object = applied("E", 1, declaration("E", &values));
        assert_eq!(
            object.fields[0].validity.as_deref(),
            Some("enum_unsigned:1")
        );
    }
}

#[test]
fn qualified_names_disambiguate_but_partial_namespace_suffixes_do_not() {
    let mut decl = json!({"kind": "TranslationUnitDecl", "inner": [
        {"kind": "NamespaceDecl", "name": "a", "inner": [declaration("E", &[Some("1")])]},
        {"kind": "NamespaceDecl", "name": "b", "inner": [declaration("E", &[Some("8")])]}
    ]});
    for source in ["E", "enum E"] {
        assert!(rejects(source, 32, decl.clone()).contains("ambiguous"));
    }
    let object = applied("a::E", 32, decl.clone());
    assert_eq!(tag(&object), Some("enum_unsigned:1"));
    let object = applied("enum b::E", 32, decl.clone());
    assert_eq!(tag(&object), Some("enum_unsigned:15"));
    decl["inner"][1]["inner"] = json!([{"kind": "TypeAliasDecl", "name": "E"}]);
    assert!(rejects("E", 32, decl.clone()).contains("ambiguous"));
    assert!(tag(&applied("b::E", 32, decl.clone())).is_none());
    let nested = json!({"kind": "NamespaceDecl", "name": "outer", "inner": [decl]});
    assert!(rejects("enum a::E", 32, nested.clone()).contains("missing EnumDecl"));
    assert!(applied("a::E", 32, nested).fields[0].validity.is_none());
    let types = json!({"kind": "TranslationUnitDecl", "inner": [
        {"kind": "TypedefDecl", "name": "Count", "type": {"qualType": "int"}},
        {"kind": "NamespaceDecl", "name": "ns", "inner": [
            {"kind": "TypeAliasDecl", "name": "Count", "type": {"qualType": "int"}}
        ]}
    ]});
    assert!(tag(&applied("Count", 32, types)).is_none());
}

#[test]
fn record_and_namespace_scopes_exclude_implicit_repeated_record_names() {
    for kind in ["CXXRecordDecl", "RecordDecl"] {
        let decl = json!({"kind": "NamespaceDecl", "name": "ns", "inner": [
            {"kind": kind, "name": "Box", "inner": [
                {"kind": kind, "name": "Box", "isImplicit": true,
                 "inner": [declaration("E", &[Some("4")])]}
            ]}
        ]});
        let object = applied("enum ns::Box::E", 32, decl);
        assert_eq!(tag(&object), Some("enum_unsigned:7"));
    }
}

#[test]
fn repeated_declarations_must_agree_and_do_not_create_alias_ambiguity() {
    let e = declaration("E", &[Some("4")]);
    let decl = json!({"kind": "TranslationUnitDecl", "inner": [e.clone(), e]});
    assert!(applied("E", 32, decl).fields[0].validity.is_some());
    let decl = json!({"kind": "TranslationUnitDecl", "inner": [
        declaration("E", &[Some("4")]), declaration("E", &[Some("8")])
    ]});
    assert!(rejects("E", 32, decl).contains("conflicting"));
}

#[test]
fn nonfixed_bitfields_use_effective_width_and_reject_narrowing() {
    for (values, width, expected) in [
        (vec![Some("0"), Some("4")], 3, "(x <= (7 : [3]))"),
        (
            vec![Some("-3"), Some("5")],
            4,
            "(x <=$ (7 : [4]) && x >=$ (-8 : [4]))",
        ),
    ] {
        let decl = ast(declaration("E", &values));
        let mut object = layout(vec![field("enum E", 32)]);
        object.fields[0].bit_offset = Some(5);
        object.fields[0].bit_width = Some(width);
        apply(&mut object, &decl).unwrap();
        assert_eq!(predicate(&object.fields[0], "x").unwrap(), expected);
        object.fields[0].bit_width = Some(width - 1);
        assert!(apply(&mut object, &decl)
            .unwrap_err()
            .to_string()
            .contains("refusing truncation"));
    }
}

#[test]
fn fixed_enum_bitfields_can_be_narrower_than_their_enumerators() {
    let mut decl = declaration("E", &[Some("255")]);
    decl["fixedUnderlyingType"] = json!({"qualType": "unsigned char"});
    let mut object = layout(vec![field("E", 32)]);
    object.fields[0].bit_offset = Some(5);
    object.fields[0].bit_width = Some(2);
    apply(&mut object, &ast(decl)).unwrap();
    assert!(object.fields[0].validity.is_none());
}

#[test]
fn signed_and_unsigned_boundaries_support_every_width_through_128() {
    for bits in 1..=128 {
        let max = (u128::MAX >> (128 - bits)).to_string();
        let object = applied("E", bits, declaration("E", &[Some(&max)]));
        assert_eq!(
            object.fields[0].validity,
            Some(format!("enum_unsigned:{max}"))
        );
        assert!(predicate(&object.fields[0], "x").is_some());
        let magnitude = 1u128 << (bits - 1);
        let (min, max) = (format!("-{magnitude}"), (magnitude - 1).to_string());
        let object = applied("E", bits, declaration("E", &[Some(&min), Some(&max)]));
        assert_eq!(
            object.fields[0].validity,
            Some(format!("enum_signed:{min}:{max}"))
        );
        assert_eq!(
            predicate(&object.fields[0], "x").unwrap(),
            format!("(x <=$ ({max} : [{bits}]) && x >=$ ({min} : [{bits}]))")
        );
    }
}

#[test]
fn numeric_and_implicit_overflows_fail_instead_of_wrapping() {
    for values in [
        vec![Some("340282366920938463463374607431768211456")],
        vec![Some("-170141183460469231731687303715884105729")],
        vec![Some("340282366920938463463374607431768211455"), None],
        vec![Some("-1"), Some("340282366920938463463374607431768211455")],
    ] {
        rejects("E", 128, declaration("E", &values));
    }
    assert!(rejects("E", 8, declaration("E", &[Some("256")])).contains("requires 9"));
    assert!(rejects("E", 8, declaration("E", &[Some("-129")])).contains("requires 9"));
}

#[test]
fn malformed_or_unevaluated_initializers_never_fall_back_to_a_literal() {
    for expression in [
        json!({"kind": "ConstantExpr"}),
        json!({"kind": "ConstantExpr", "value": "bad", "inner": [{"kind": "IntegerLiteral", "value": "1"}]}),
        json!({"kind": "BinaryOperator", "inner": [{"kind": "IntegerLiteral", "value": "7"}]}),
        json!({"kind": "DeclRefExpr", "referencedDecl": {"name": "a"}}),
        json!({"kind": "IntegerLiteral", "value": 1.5}),
        json!({"kind": "ConstantExpr", "value": false}),
        json!({"kind": "UnaryOperator", "opcode": "-"}),
        json!({"kind": "UnaryOperator", "opcode": "~", "inner": [{"kind": "IntegerLiteral", "value": "1"}]}),
        json!({"kind": "UnaryOperator", "opcode": "-", "type": {"qualType": "unsigned int"}, "inner": [{"kind": "IntegerLiteral", "value": "1"}]}),
        json!({"kind": "CStyleCastExpr", "inner": [{"kind": "IntegerLiteral", "value": "257"}]}),
    ] {
        let decl = json!({"kind": "EnumDecl", "name": "E", "inner": [
            {"kind": "EnumConstantDecl", "name": "a", "inner": [expression]}
        ]});
        rejects("E", 32, decl);
    }
    for decl in [
        json!({"kind": "EnumDecl", "name": "E", "scopedEnumTag": false}),
        json!({"kind": "EnumDecl", "name": "E", "isInvalid": true}),
        json!({"kind": "EnumDecl", "name": "E", "inner": [{"kind": "IntegerLiteral", "value": "1"}]}),
        json!({"kind": "EnumDecl", "name": "E", "inner": [{"kind": "EnumConstantDecl", "name": "a", "hasInit": true}]}),
    ] {
        rejects("E", 32, decl);
    }
}

#[test]
fn explicit_missing_enums_fail_while_unknown_typedefs_warn_and_runtime_is_skipped() {
    assert!(rejects("const enum Missing", 32, json!({})).contains("missing EnumDecl"));
    let mut fields = vec![
        field("OpaqueTypedef", 32),
        field("enum SystemEnum", 32),
        field("enum Pointee *", 64),
        field("bool", 8),
    ];
    fields[1].runtime = true;
    fields[2].is_pointer = true;
    fields[3].validity = Some("bool".into());
    let mut object = layout(fields.clone());
    apply(&mut object, &AstNode::default()).unwrap();
    assert_eq!(object.fields, fields);
    assert_eq!(object.validation.len(), 1);
    assert!(object.validation[0].contains("typedef or filtered AST"));
}

#[test]
fn changes_are_limited_idempotent_and_transactional_for_fields() {
    let decl = ast(declaration("E", &[Some("4")]));
    let mut object = layout(vec![field("E", 32)]);
    object.fields[0].guard = Some("engaged".into());
    object.validation.push("user constraint: value == 4".into());
    object.unresolved.push("unrelated storage fact".into());
    let original = object.clone();
    apply(&mut object, &decl).unwrap();
    let once = serde_json::to_value(&object).unwrap();
    apply(&mut object, &decl).unwrap();
    assert_eq!(serde_json::to_value(&object).unwrap(), once);
    object.fields[0].validity = original.fields[0].validity.clone();
    object.validation = original.validation.clone();
    assert_eq!(
        serde_json::to_value(&object).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    object.fields.push(field("enum Missing", 32));
    let fields = object.fields.clone();
    assert!(apply(&mut object, &decl).is_err());
    assert_eq!(object.fields, fields);
    assert_eq!(object.unresolved, original.unresolved);
}

#[test]
fn predicates_reject_invalid_tags_and_widths_and_preserve_boolean_validity() {
    let mut field = field("bool", 8);
    field.validity = Some("bool".into());
    assert_eq!(predicate(&field, "x").unwrap(), "(x <= (1 : [8]))");
    for tag in [
        "other",
        "enum_unsigned:-1",
        "enum_unsigned:256",
        "enum_signed:-129:127",
        "enum_signed:-1:128",
        "enum_signed:0:1",
        "enum_signed:-1:0:1",
    ] {
        field.validity = Some(tag.into());
        assert!(predicate(&field, "x").is_none(), "accepted {tag}");
    }
    field.validity = Some("bool".into());
    for ty in ["i0", "i129", "i+32", "i", "float", "ptr"] {
        field.llvm_type = ty.into();
        assert!(predicate(&field, "x").is_none());
    }
    field.llvm_type = "i8".into();
    field.bit_width = Some(1);
    field.bit_offset = Some(0);
    assert_eq!(predicate(&field, "x").unwrap(), "(x <= (1 : [1]))");
    field.bit_offset = Some(usize::MAX);
    assert!(predicate(&field, "x").is_none());
}
