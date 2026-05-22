//! Function and parameter extraction.
//!
//! Drives the multi-pass pipeline behind [`extract_functions`]:
//!   1. Build a [`TypeContext`] for type resolution.
//!   2. Build a node-id → class-name map for `parentDeclContextId` lookups.
//!   3. Collect every `FunctionDecl`/`CXXMethodDecl` via the visitor.
//!   4. Resolve referenced globals + called functions.

use super::calls::resolve_called_functions;
use super::cpp_types::{parse_cpp_type, parse_return_type, strip_restrict};
use super::globals::{collect_globals, resolve_referenced_globals};
use super::node::AstNode;
use super::sal::parse_sal_annotation;
use super::system_headers::is_system_include_path;
use super::type_ctx::{build as build_type_ctx, build_node_id_map, TypeContext};
use super::visitor::{walk, ClassStack, Visitor, WalkAction};
use crate::constraints::{
    Annotation, FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo,
};
use anyhow::Result;
use std::collections::HashMap;

/// Extract every function declaration from `ast`, optionally filtered by
/// name substring. Returns enriched [`FunctionInfo`]s with resolved
/// parameters, return type, annotations, referenced globals, and called
/// functions.
pub fn extract_functions(ast: &AstNode, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let ctx = build_type_ctx(ast);
    let id_to_name = build_node_id_map(ast);
    let global_index = collect_globals(ast, &ctx);

    let mut functions = Vec::new();
    let mut fv = FunctionVisitor {
        out: &mut functions,
        filter,
        ctx: &ctx,
        id_to_name: &id_to_name,
        stack: ClassStack::new(),
    };
    walk(ast, &mut fv);

    resolve_referenced_globals(ast, &mut functions, &global_index);
    resolve_called_functions(ast, &mut functions);
    Ok(functions)
}

/// Visitor that turns each `FunctionDecl` / `CXXMethodDecl` into a
/// [`FunctionInfo`]. Maintains an enclosing-class stack so methods of
/// nested classes can resolve their parent name even when clang omits
/// `parentDeclContextId`.
pub(super) struct FunctionVisitor<'a> {
    pub out: &'a mut Vec<FunctionInfo>,
    pub filter: Option<&'a str>,
    pub ctx: &'a TypeContext,
    pub id_to_name: &'a HashMap<String, String>,
    pub stack: ClassStack,
}

impl<'a> Visitor for FunctionVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        match node.kind.as_str() {
            "FunctionDecl" | "CXXMethodDecl" => {
                if let Some(f) = parse_function_decl(
                    node,
                    node.kind == "CXXMethodDecl",
                    self.ctx,
                    self.id_to_name,
                    self.stack.current(),
                ) {
                    if self.filter.map(|f2| f.name.contains(f2)).unwrap_or(true) {
                        self.out.push(f);
                    }
                }
            }
            "FunctionTemplateDecl" => {
                // Extract any instantiated specializations under the template.
                for child in &node.inner {
                    if matches!(child.kind.as_str(), "FunctionDecl" | "CXXMethodDecl") {
                        if let Some(f) = parse_function_decl(
                            child,
                            child.kind == "CXXMethodDecl",
                            self.ctx,
                            self.id_to_name,
                            self.stack.current(),
                        ) {
                            if self.filter.map(|f2| f.name.contains(f2)).unwrap_or(true) {
                                self.out.push(f);
                            }
                        }
                    }
                }
                // Don't descend — we handled the relevant children above.
                self.stack.push_if_class(node);
                return WalkAction::SkipChildren;
            }
            _ => {}
        }
        self.stack.push_if_class(node);
        WalkAction::Continue
    }
    fn leave(&mut self, node: &AstNode) {
        self.stack.pop_if(node.class_name().is_some());
    }
}

/// Parse one `FunctionDecl` / `CXXMethodDecl` node into a [`FunctionInfo`].
pub fn parse_function_decl(
    node: &AstNode,
    is_method: bool,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
    enclosing_class: Option<&str>,
) -> Option<FunctionInfo> {
    let name = node.name.clone()?;
    let mangled = node.mangled_name.clone();
    let qual_type = node.qual_type()?.to_string();

    // C++ method constness: `const` appears AFTER the parameter list,
    // never inside it. Locate it relative to the last closing paren.
    let is_const_method = is_method && {
        let after_paren = qual_type
            .rfind(')')
            .map(|pos| &qual_type[pos + 1..])
            .unwrap_or("");
        after_paren.contains("const")
    };
    let mut is_noexcept = qual_type.contains("noexcept");

    // Look at children for explicit attributes and ExceptionSpec.
    for child in &node.inner {
        if matches!(child.kind.as_str(), "NoThrowAttr" | "NoExceptAttr") {
            is_noexcept = true;
        }
        if let Some(spec) = child.exception_spec.as_ref() {
            if matches!(spec.r#type.as_deref(), Some("noexcept") | Some("nothrow")) {
                is_noexcept = true;
            }
        }
    }

    let mut params = Vec::new();
    let mut func_annotations = Vec::new();
    if is_noexcept {
        func_annotations.push(Annotation::NoThrow);
    }

    if is_method {
        params.push(synth_this_param(node, ctx, id_to_name, enclosing_class, is_const_method));
    }
    for child in &node.inner {
        if child.kind == "ParmVarDecl" {
            if let Some(p) = parse_param(child, ctx) {
                params.push(p);
            }
        }
    }

    let return_type = parse_return_type(&qual_type, ctx);
    let is_virtual = node.r#virtual == Some(true);
    let has_body = node.inner.iter().any(|c| c.kind == "CompoundStmt");
    let is_system = node
        .loc
        .as_ref()
        .and_then(|l| l.included_from.as_deref())
        .and_then(|i| i.file.as_deref())
        .map(is_system_include_path)
        .unwrap_or(false);

    Some(FunctionInfo {
        name,
        mangled_name: mangled,
        params,
        return_type,
        can_throw: !is_noexcept,
        is_virtual,
        has_body,
        is_system,
        annotations: func_annotations,
        referenced_globals: vec![],
        called_functions: vec![],
    })
}

/// Synthesize the implicit `this` parameter for a C++ method.
fn synth_this_param(
    node: &AstNode,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
    enclosing_class: Option<&str>,
    is_const_method: bool,
) -> ParamInfo {
    let parent_id = node.parent_decl_context_id.as_deref().unwrap_or("");
    let parent_name: &str = id_to_name
        .get(parent_id)
        .map(|s| s.as_str())
        .or(enclosing_class)
        .unwrap_or("Self");
    let inner_type = TypeInfo::Opaque {
        name: "Self".into(),
        size_bytes: 0,
    };
    // A const method normally promises not to write through `this`,
    // BUT the `mutable` keyword (and `const_cast`) means a const method
    // can still legally modify mutable members. Stay sound: if the
    // class (or any base) has a `mutable` field, treat `this` as
    // Mutable.
    let mutability = if is_const_method
        && !ctx.classes_with_mutable_field.contains(parent_name)
    {
        Mutability::Readonly
    } else {
        Mutability::Mutable
    };
    ParamInfo {
        name: "this".into(),
        ty: TypeInfo::Pointer(Box::new(inner_type)),
        mutability,
        nullable: Nullability::NonNull,
        annotations: Vec::new(),
    }
}

/// Parse one `ParmVarDecl` into a [`ParamInfo`]. Combines C++ type-string
/// information with SAL annotations: the strictest mutability wins.
pub fn parse_param(node: &AstNode, ctx: &TypeContext) -> Option<ParamInfo> {
    let name = node.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let qual_type_raw = node.qual_type()?;
    let qual_type_owned = strip_restrict(qual_type_raw);
    let qual_type = qual_type_owned.as_str();

    let is_const = qual_type.contains("const");
    let is_ref = qual_type.contains('&');
    let is_ptr = qual_type.contains('*');

    let mut mutability = if is_const && (is_ref || is_ptr) {
        Mutability::Readonly
    } else if is_ref || is_ptr {
        Mutability::Mutable
    } else {
        // Pass-by-value: effectively read-only from the caller's perspective.
        Mutability::Readonly
    };
    let nullable = if is_ref {
        Nullability::NonNull
    } else if is_ptr {
        Nullability::Nullable
    } else {
        Nullability::NonNull
    };

    // Pull SAL annotations from attribute children.
    let mut annotations = Vec::new();
    for attr in &node.inner {
        if let Some(a) = parse_sal_annotation(attr) {
            annotations.push(a);
        }
    }

    // SAL + type system: strictest constraint wins. `const` in the type
    // beats any SAL annotation; otherwise SAL can tighten Mutable into
    // Readonly/WriteOnly.
    mutability = annotations.iter().fold(mutability, |m, ann| match ann {
        Annotation::InReads(_) => Mutability::Readonly,
        Annotation::OutWrites(_) => {
            if m == Mutability::Readonly {
                Mutability::Readonly
            } else {
                Mutability::WriteOnly
            }
        }
        Annotation::Inout => {
            if m == Mutability::Readonly {
                Mutability::Readonly
            } else {
                Mutability::Mutable
            }
        }
        _ => m,
    });

    let ty = parse_cpp_type(qual_type, ctx);
    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations,
    })
}

#[cfg(test)]
mod tests {
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
}
