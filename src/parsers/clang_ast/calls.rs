//! Per-function `called_functions` resolution pass.
//!
//! Walks every function body looking for `CallExpr`/`CXXMemberCallExpr`/
//! `CXXOperatorCallExpr`/`CXXConstructExpr` plus `CXXNewExpr.operatorNewDecl`
//! and `CXXDeleteExpr.operatorDeleteDecl`, then unifies the callee names
//! against an AST-wide function ID map so MSVC-mangled names match
//! across translation units.

use super::node::AstNode;
use super::visitor::{walk, Visitor, WalkAction};
use crate::constraints::{CalledFunction, FunctionInfo};
use std::collections::HashMap;

/// Resolve `called_functions` for every entry in `functions`.
pub fn resolve_called_functions(ast: &AstNode, functions: &mut [FunctionInfo]) {
    let id_to_fn = build_function_id_map(ast);
    let mut v = CallResolver {
        functions,
        id_to_fn: &id_to_fn,
    };
    walk(ast, &mut v);
}

/// Build an AST-wide `node_id → (name, mangled_name)` map for every
/// function-like decl. Used to canonicalize `referencedMemberDecl` /
/// inline `referencedDecl` summaries against real definitions.
fn build_function_id_map(ast: &AstNode) -> HashMap<String, (String, String)> {
    struct V<'a>(&'a mut HashMap<String, (String, String)>);
    impl<'a> Visitor for V<'a> {
        fn enter(&mut self, n: &AstNode) -> WalkAction {
            if !n.is_function_like() {
                return WalkAction::Continue;
            }
            let id = n.id.as_deref().unwrap_or("");
            let name = n.name.as_deref().unwrap_or("");
            let mangled = n.mangled_name.as_deref().unwrap_or("");
            if !id.is_empty() && (!name.is_empty() || !mangled.is_empty()) {
                let m = if mangled.is_empty() { name } else { mangled };
                self.0
                    .insert(id.to_string(), (name.to_string(), m.to_string()));
            }
            WalkAction::Continue
        }
    }
    let mut out = HashMap::new();
    let mut v = V(&mut out);
    walk(ast, &mut v);
    out
}

struct CallResolver<'a> {
    functions: &'a mut [FunctionInfo],
    id_to_fn: &'a HashMap<String, (String, String)>,
}

impl<'a> Visitor for CallResolver<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if !node.is_function_like() {
            return WalkAction::Continue;
        }
        let fname = node.name.as_deref().unwrap_or("");
        let mangled = node.mangled_name.as_deref();
        let mut calls = Vec::new();
        collect_call_refs(node, &mut calls, self.id_to_fn);
        calls.sort_by(|a, b| a.mangled_name.cmp(&b.mangled_name));
        calls.dedup_by(|a, b| a.mangled_name == b.mangled_name);
        if let Some(func) =
            self.functions
                .iter_mut()
                .find(|f| match (mangled, f.mangled_name.as_deref()) {
                    (Some(a), Some(b)) => a == b,
                    _ => f.name == fname,
                })
        {
            func.called_functions = calls;
        }
        WalkAction::Continue
    }
}

/// Recursively collect every call/construct/new/delete-style reference
/// under `node`.
fn collect_call_refs(
    node: &AstNode,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    match node.kind.as_str() {
        "CallExpr" | "CXXMemberCallExpr" | "CXXOperatorCallExpr" | "CXXConstructExpr" => {
            if let Some(rd) = node.referenced_decl.as_deref() {
                push_called_decl(rd, out, id_to_fn);
            }
            collect_callee_ids(node, out, id_to_fn);
            for child in &node.inner {
                if matches!(
                    child.kind.as_str(),
                    "DeclRefExpr" | "ImplicitCastExpr" | "MemberExpr"
                ) {
                    collect_callee_from_expr(child, out, id_to_fn);
                }
            }
        }
        "CXXNewExpr" => {
            if let Some(op) = node.operator_new_decl.as_deref() {
                push_called_decl(op, out, id_to_fn);
            }
        }
        "CXXDeleteExpr" => {
            if let Some(op) = node.operator_delete_decl.as_deref() {
                push_called_decl(op, out, id_to_fn);
            }
        }
        _ => {}
    }
    for child in &node.inner {
        collect_call_refs(child, out, id_to_fn);
    }
}

/// Walk a call/construct expression looking for ID-only callee references
/// stored in scalar string fields (`referencedMemberDecl`, etc.).
fn collect_callee_ids(
    node: &AstNode,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    let ids = [
        node.referenced_member_decl.as_deref(),
        node.constructor_decl.as_deref(),
        node.destructor_decl.as_deref(),
    ];
    for id in ids.iter().flatten() {
        if let Some((name, mangled)) = id_to_fn.get(*id) {
            out.push(CalledFunction {
                name: name.clone(),
                mangled_name: mangled.clone(),
                has_body: false,
            });
        }
    }
    for child in &node.inner {
        collect_callee_ids(child, out, id_to_fn);
    }
}

/// Resolve a `referencedDecl` summary into a [`CalledFunction`], preferring
/// the canonical mangled name from `id_to_fn` (the inline summary often
/// lacks `mangledName`).
fn push_called_decl(
    decl: &AstNode,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    let id = decl.id.as_deref().unwrap_or("");
    let summary_name = decl.name.as_deref().unwrap_or("");
    let summary_mangled = decl.mangled_name.as_deref().unwrap_or("");
    let (name, mangled) = if !id.is_empty() {
        if let Some((n, m)) = id_to_fn.get(id) {
            (n.clone(), m.clone())
        } else {
            fallback_pair(summary_name, summary_mangled)
        }
    } else {
        fallback_pair(summary_name, summary_mangled)
    };
    if name.is_empty() && mangled.is_empty() {
        return;
    }
    out.push(CalledFunction {
        name,
        mangled_name: mangled,
        has_body: false,
    });
}

fn fallback_pair(name: &str, mangled: &str) -> (String, String) {
    let m = if mangled.is_empty() { name } else { mangled };
    (name.to_string(), m.to_string())
}

/// Recurse through cast/decl-ref/member expressions resolving every
/// embedded `referencedDecl`.
fn collect_callee_from_expr(
    expr: &AstNode,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    if matches!(expr.kind.as_str(), "DeclRefExpr" | "MemberExpr") {
        if let Some(rd) = expr.referenced_decl.as_deref() {
            push_called_decl(rd, out, id_to_fn);
        }
    }
    for child in &expr.inner {
        collect_callee_from_expr(child, out, id_to_fn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::TypeInfo;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    fn stub_fn(name: &str, mangled: Option<&str>) -> FunctionInfo {
        FunctionInfo {
            name: name.into(),
            mangled_name: mangled.map(String::from),
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: true,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    #[test]
    fn resolves_direct_call_expr() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "id": "0xCallee", "name": "callee",
                 "mangledName": "?callee@@YAXXZ", "type": {"qualType": "void ()"}},
                {"kind": "FunctionDecl", "name": "caller",
                 "mangledName": "?caller@@YAXXZ", "type": {"qualType": "void ()"},
                 "inner": [{"kind": "CompoundStmt", "inner": [
                     {"kind": "CallExpr", "referencedDecl":
                         {"kind": "FunctionDecl", "id": "0xCallee", "name": "callee"}}
                 ]}]}
            ]
        }));
        let mut funcs = vec![stub_fn("caller", Some("?caller@@YAXXZ"))];
        resolve_called_functions(&ast, &mut funcs);
        assert_eq!(funcs[0].called_functions.len(), 1);
        assert_eq!(funcs[0].called_functions[0].name, "callee");
        assert_eq!(funcs[0].called_functions[0].mangled_name, "?callee@@YAXXZ");
    }

    #[test]
    fn dedups_repeated_calls_to_same_callee() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "id": "0xC", "name": "c",
                 "mangledName": "_Z1cv", "type": {"qualType": "void ()"}},
                {"kind": "FunctionDecl", "name": "caller",
                 "mangledName": "_Z6callerv", "type": {"qualType": "void ()"},
                 "inner": [{"kind": "CompoundStmt", "inner": [
                     {"kind": "CallExpr", "referencedDecl": {"kind": "FunctionDecl", "id": "0xC"}},
                     {"kind": "CallExpr", "referencedDecl": {"kind": "FunctionDecl", "id": "0xC"}}
                 ]}]}
            ]
        }));
        let mut funcs = vec![stub_fn("caller", Some("_Z6callerv"))];
        resolve_called_functions(&ast, &mut funcs);
        assert_eq!(funcs[0].called_functions.len(), 1);
    }

    #[test]
    fn resolves_id_only_member_call() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "id": "0xM", "name": "m",
                 "mangledName": "?m@C@@QEAAXXZ", "type": {"qualType": "void ()"}},
                {"kind": "FunctionDecl", "name": "caller",
                 "type": {"qualType": "void ()"},
                 "inner": [{"kind": "CompoundStmt", "inner": [
                     {"kind": "CXXMemberCallExpr",
                      "referencedMemberDecl": "0xM"}
                 ]}]}
            ]
        }));
        let mut funcs = vec![stub_fn("caller", None)];
        resolve_called_functions(&ast, &mut funcs);
        assert_eq!(funcs[0].called_functions.len(), 1);
        assert_eq!(funcs[0].called_functions[0].name, "m");
    }

    #[test]
    fn resolves_operator_new_and_delete() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "id": "0xN", "name": "operator new",
                 "mangledName": "??2@YAPEAX_K@Z", "type": {"qualType": "void* (unsigned long long)"}},
                {"kind": "FunctionDecl", "id": "0xD", "name": "operator delete",
                 "mangledName": "??3@YAXPEAX@Z", "type": {"qualType": "void (void*)"}},
                {"kind": "FunctionDecl", "name": "alloc_and_free",
                 "type": {"qualType": "void ()"},
                 "inner": [{"kind": "CompoundStmt", "inner": [
                     {"kind": "CXXNewExpr",
                      "operatorNewDecl": {"kind": "FunctionDecl", "id": "0xN"}},
                     {"kind": "CXXDeleteExpr",
                      "operatorDeleteDecl": {"kind": "FunctionDecl", "id": "0xD"}}
                 ]}]}
            ]
        }));
        let mut funcs = vec![stub_fn("alloc_and_free", None)];
        resolve_called_functions(&ast, &mut funcs);
        let names: Vec<&str> = funcs[0]
            .called_functions
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.iter().any(|n| *n == "operator new"));
        assert!(names.iter().any(|n| *n == "operator delete"));
    }
}
