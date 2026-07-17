//! Tests for [`super::functions`].
//!
//! Lifted out of `functions.rs` to keep that file under the 500-
//! non-whitespace-line repo limit. See `.github/copilot-instructions.md`.

use super::*;
use serde_json::json;

fn parse(v: serde_json::Value) -> AstNode {
    serde_json::from_value(v).unwrap()
}

#[test]
fn extract_simple_function_with_params() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionDecl",
            "name": "add",
            "type": {"qualType": "int (int, int)"},
            "inner": [
                {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "int"}},
                {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "int"}}
            ]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].name, "add");
    assert_eq!(funcs[0].params.len(), 2);
    assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
}

#[test]
fn filter_limits_by_substring() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {"kind": "FunctionDecl", "name": "validate", "type": {"qualType": "void ()"}},
            {"kind": "FunctionDecl", "name": "process", "type": {"qualType": "void ()"}}
        ]
    }));
    let funcs = extract_functions(&ast, Some("valid")).unwrap();
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].name, "validate");
}

#[test]
fn const_method_gets_readonly_this() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "CXXRecordDecl",
            "name": "MyClass",
            "id": "0xMC",
            "inner": [{
                "kind": "CXXMethodDecl",
                "name": "getValue",
                "parentDeclContextId": "0xMC",
                "type": {"qualType": "int () const"}
            }]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs[0].params[0].name, "this");
    assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
}

#[test]
fn non_const_method_gets_mutable_this() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "CXXRecordDecl",
            "name": "MyClass",
            "id": "0xMC",
            "inner": [{
                "kind": "CXXMethodDecl",
                "name": "setValue",
                "parentDeclContextId": "0xMC",
                "type": {"qualType": "void (int)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "v", "type": {"qualType": "int"}}
                ]
            }]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs[0].params[0].mutability, Mutability::Mutable);
}

#[test]
fn by_value_aggregate_param_is_mutable() {
    // MSVC lowers a by-value class param into a hidden indirect pointer
    // that the callee OWNS and may mutate (e.g. reset a field before
    // moving it into a member). Its SAW backing allocation must be
    // MUTABLE, otherwise any such store drives the proof vacuous.
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionDecl",
            "name": "provision",
            "type": {"qualType": "void (EnrollmentKey)"},
            "inner": [
                {"kind": "ParmVarDecl", "name": "newKey",
                 "type": {"qualType": "EnrollmentKey"}}
            ]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs[0].params[0].mutability, Mutability::Mutable);
}

#[test]
fn by_value_scalar_param_is_readonly() {
    // A by-value scalar passes in a register and is lowered to a fresh
    // symbolic value, so its mutability flag stays Readonly (inert).
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionDecl",
            "name": "square",
            "type": {"qualType": "int (int)"},
            "inner": [
                {"kind": "ParmVarDecl", "name": "n", "type": {"qualType": "int"}}
            ]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
}

#[test]
fn noexcept_in_qualtype_clears_can_throw() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionDecl",
            "name": "safe",
            "type": {"qualType": "void () noexcept"}
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert!(!funcs[0].can_throw);
    assert!(funcs[0]
        .annotations
        .iter()
        .any(|a| matches!(a, Annotation::NoThrow)));
}

#[test]
fn nothrow_attribute_clears_can_throw() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionDecl",
            "name": "safe2",
            "type": {"qualType": "void ()"},
            "inner": [{"kind": "NoThrowAttr"}]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert!(!funcs[0].can_throw);
}

#[test]
fn template_specialization_is_extracted() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [{
            "kind": "FunctionTemplateDecl",
            "name": "max",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "max",
                "type": {"qualType": "int (int, int)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "int"}},
                    {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "int"}}
                ]
            }]
        }]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].name, "max");
}

/// Two C++ overloads sharing the friendly name `add` but with different
/// parameter types must both survive extraction. Losing one would silently
/// drop a function from spec generation.
#[test]
fn extract_keeps_both_overloads() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "FunctionDecl",
                "name": "add",
                "mangledName": "_Z3addii",
                "type": {"qualType": "int (int, int)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "int"}},
                    {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "int"}}
                ]
            },
            {
                "kind": "FunctionDecl",
                "name": "add",
                "mangledName": "_Z3adddd",
                "type": {"qualType": "double (double, double)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "double"}},
                    {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "double"}}
                ]
            }
        ]
    }));
    let funcs = extract_functions(&ast, None).unwrap();
    assert_eq!(funcs.len(), 2, "both overloads must be returned");
    assert!(funcs.iter().all(|f| f.name == "add"));
    // Distinct mangled names ensure downstream identifiers (filenames,
    // SAW variable names) stay collision-free.
    let mangled: Vec<&str> = funcs
        .iter()
        .filter_map(|f| f.mangled_name.as_deref())
        .collect();
    assert_eq!(mangled.len(), 2);
    assert_ne!(mangled[0], mangled[1]);
    // Return types must differ — the int overload returns SignedInt(32),
    // the double overload returns Float(64) (or similar) — they must not
    // be identical or one overload has overwritten the other.
    assert_ne!(funcs[0].return_type, funcs[1].return_type);
}

/// `--filter add` must return ALL overloads whose name contains "add",
/// not just the first match. This is what `from-clang-ast --filter` relies
/// on to emit one spec per overload.
#[test]
fn filter_substring_matches_all_overloads() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "FunctionDecl",
                "name": "add",
                "mangledName": "_Z3addii",
                "type": {"qualType": "int (int, int)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "int"}},
                    {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "int"}}
                ]
            },
            {
                "kind": "FunctionDecl",
                "name": "add",
                "mangledName": "_Z3adddd",
                "type": {"qualType": "double (double, double)"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "a", "type": {"qualType": "double"}},
                    {"kind": "ParmVarDecl", "name": "b", "type": {"qualType": "double"}}
                ]
            },
            {
                "kind": "FunctionDecl",
                "name": "subtract",
                "type": {"qualType": "int (int, int)"}
            }
        ]
    }));
    let funcs = extract_functions(&ast, Some("add")).unwrap();
    assert_eq!(funcs.len(), 2, "filter must keep both add overloads");
    assert!(funcs.iter().all(|f| f.name == "add"));
}

/// Same friendly name on different classes — common in C++. Each method
/// must surface independently with its own mangled symbol so it can be
/// matched against the LLVM bitcode.
#[test]
fn methods_on_different_classes_with_same_name_kept_separate() {
    let ast = parse(json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "CXXRecordDecl",
                "name": "Foo",
                "id": "0xFOO",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "compute",
                    "mangledName": "_ZNK3Foo7computeEv",
                    "parentDeclContextId": "0xFOO",
                    "type": {"qualType": "int () const"}
                }]
            },
            {
                "kind": "CXXRecordDecl",
                "name": "Bar",
                "id": "0xBAR",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "compute",
                    "mangledName": "_ZNK3Bar7computeEv",
                    "parentDeclContextId": "0xBAR",
                    "type": {"qualType": "int () const"}
                }]
            }
        ]
    }));
    let funcs = extract_functions(&ast, Some("compute")).unwrap();
    assert_eq!(funcs.len(), 2);
    let mangled: Vec<&str> = funcs
        .iter()
        .filter_map(|f| f.mangled_name.as_deref())
        .collect();
    assert_eq!(mangled.len(), 2);
    assert_ne!(mangled[0], mangled[1]);
}
