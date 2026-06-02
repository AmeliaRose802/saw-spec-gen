//! Top-level global variable extraction and the per-function
//! `referenced_globals` resolution pass.

use super::cpp_types::parse_cpp_type;
use super::node::AstNode;
use super::type_ctx::{build as build_type_ctx, TypeContext};
use super::visitor::{walk, Visitor, WalkAction};
use crate::constraints::{FunctionInfo, GlobalVarInfo};
use anyhow::Result;
use std::collections::HashMap;

/// Map from clang node id ("0x...") to the resolved global info.
pub type GlobalIndex = HashMap<String, GlobalVarInfo>;

/// Extract all file-scope global variables from the AST.
pub fn extract_all_globals(ast: &AstNode) -> Result<Vec<GlobalVarInfo>> {
    let ctx = build_type_ctx(ast);
    let map = collect_globals(ast, &ctx);
    Ok(map.into_values().collect())
}

/// Build a `node_id → GlobalVarInfo` index from top-level `VarDecl`s.
/// Skips extern declarations and `const`-qualified globals (the compiler
/// inlines those; they don't appear in bitcode).
pub fn collect_globals(ast: &AstNode, ctx: &TypeContext) -> GlobalIndex {
    let mut globals = GlobalIndex::new();
    for node in &ast.inner {
        if node.kind != "VarDecl" {
            continue;
        }
        if node.storage_class.as_deref() == Some("extern") {
            continue;
        }
        let qual_type_raw = node.qual_type().unwrap_or("");
        if qual_type_raw.starts_with("const ") || qual_type_raw.contains(" const") {
            continue;
        }
        let name = match node.name.as_deref() {
            Some(n) => n,
            None => continue,
        };
        let mangled = match node.mangled_name.as_deref() {
            Some(m) => m,
            None => continue,
        };
        let qual_type = if qual_type_raw.is_empty() {
            "int"
        } else {
            qual_type_raw
        };
        let ty = parse_cpp_type(qual_type, ctx);
        let id = node.id.as_deref().unwrap_or("");
        let init_value = node.inner.iter().find_map(|c| {
            if c.kind == "IntegerLiteral" {
                c.value.as_ref().and_then(|v| v.as_str()).map(String::from)
            } else {
                None
            }
        });
        globals.insert(
            id.to_string(),
            GlobalVarInfo {
                name: name.to_string(),
                mangled_name: mangled.to_string(),
                ty,
                init_value,
            },
        );
    }
    globals
}

/// Walk every function body looking for `DeclRefExpr`s that reference
/// a known global, and populate the matching [`FunctionInfo`]'s
/// `referenced_globals`.
pub fn resolve_referenced_globals(
    ast: &AstNode,
    functions: &mut [FunctionInfo],
    globals: &GlobalIndex,
) {
    if globals.is_empty() {
        return;
    }
    let mut v = GlobalResolver { functions, globals };
    walk(ast, &mut v);
}

struct GlobalResolver<'a> {
    functions: &'a mut [FunctionInfo],
    globals: &'a GlobalIndex,
}

impl<'a> Visitor for GlobalResolver<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if !matches!(node.kind.as_str(), "FunctionDecl" | "CXXMethodDecl") {
            return WalkAction::Continue;
        }
        let fname = node.name.as_deref().unwrap_or("");
        let mangled = node.mangled_name.as_deref();
        let mut ids = Vec::new();
        collect_global_ids(node, &mut ids, self.globals);
        if ids.is_empty() {
            return WalkAction::Continue;
        }
        ids.sort();
        ids.dedup();
        if let Some(func) =
            self.functions
                .iter_mut()
                .find(|f| match (mangled, f.mangled_name.as_deref()) {
                    (Some(a), Some(b)) => a == b,
                    _ => f.name == fname,
                })
        {
            for id in ids {
                if let Some(g) = self.globals.get(&id) {
                    if !func
                        .referenced_globals
                        .iter()
                        .any(|rg| rg.mangled_name == g.mangled_name)
                    {
                        func.referenced_globals.push(g.clone());
                    }
                }
            }
        }
        WalkAction::Continue
    }
}

/// Recursively collect global variable reference IDs from `DeclRefExpr`
/// nodes whose `referencedDecl` is a `VarDecl` we know about.
fn collect_global_ids(node: &AstNode, out: &mut Vec<String>, globals: &GlobalIndex) {
    if node.kind == "DeclRefExpr" {
        if let Some(rd) = node.referenced_decl.as_deref() {
            if rd.kind == "VarDecl" {
                if let Some(id) = rd.id.as_deref() {
                    if globals.contains_key(id) {
                        out.push(id.to_string());
                    }
                }
            }
        }
    }
    for child in &node.inner {
        collect_global_ids(child, out, globals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn picks_up_simple_global_with_initializer() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "VarDecl",
                "id": "0x1",
                "name": "counter",
                "mangledName": "?counter@@3HA",
                "type": {"qualType": "int"},
                "inner": [{"kind": "IntegerLiteral", "value": "0"}]
            }]
        }));
        let g = extract_all_globals(&ast).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].name, "counter");
        assert_eq!(g[0].init_value.as_deref(), Some("0"));
    }

    #[test]
    fn skips_const_and_extern() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "VarDecl", "id": "0x1", "name": "K", "mangledName": "K",
                 "type": {"qualType": "const int"}},
                {"kind": "VarDecl", "id": "0x2", "name": "ext", "mangledName": "ext",
                 "type": {"qualType": "int"}, "storageClass": "extern"}
            ]
        }));
        let g = extract_all_globals(&ast).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn resolve_links_function_to_referenced_global() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "VarDecl", "id": "0xG", "name": "g", "mangledName": "g",
                 "type": {"qualType": "int"}},
                {"kind": "FunctionDecl", "name": "use_g", "type": {"qualType": "int ()"},
                 "inner": [
                     {"kind": "CompoundStmt", "inner": [
                         {"kind": "DeclRefExpr", "referencedDecl":
                             {"kind": "VarDecl", "id": "0xG"}}
                     ]}
                 ]}
            ]
        }));
        let ctx = build_type_ctx(&ast);
        let idx = collect_globals(&ast, &ctx);
        let mut funcs = vec![FunctionInfo {
            name: "use_g".into(),
            mangled_name: None,
            params: vec![],
            return_type: crate::constraints::TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }];
        resolve_referenced_globals(&ast, &mut funcs, &idx);
        assert_eq!(funcs[0].referenced_globals.len(), 1);
        assert_eq!(funcs[0].referenced_globals[0].name, "g");
    }
}
