//! Recover source declaration names and exact, positional ABI arguments.

use crate::clang_ast::AstNode;
use crate::object_layout::clang::normalize_name;
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct SourceSignature {
    pub receiver: Option<String>,
    pub params: BTreeMap<String, String>,
    pub return_type: String,
}

// Share canonical scope spelling with abstract-record lookup. Injected class
// names do not introduce another scope; local classes need a different naming
// scheme and must not be mistaken for namespace/global records.
fn walk_scoped<'a>(
    node: &'a AstNode,
    scope: &str,
    names: &mut BTreeMap<String, String>,
    visit: &mut impl FnMut(&'a AstNode, &str),
) {
    let mut scope = scope.to_owned();
    let is_scope = node.is_record() || node.kind == "NamespaceDecl";
    if is_scope && node.is_implicit != Some(true) {
        if let Some(parent) = &node.parent_decl_context_id {
            let Some(parent_scope) = names.get(parent) else {
                return; // An unresolved semantic context is not a lexical guess.
            };
            scope = parent_scope.clone();
        }
        let Some(name) = scope_component(node) else {
            return;
        };
        scope = if let Some(name) = name.strip_prefix("::") {
            name.into()
        } else if scope.is_empty() {
            name
        } else {
            format!("{scope}::{name}")
        };
    }
    if is_scope || node.kind == "TranslationUnitDecl" {
        if let Some(id) = &node.id {
            names.entry(id.clone()).or_insert_with(|| scope.clone());
        }
    }
    visit(node, &scope);
    if !node.is_function_like() {
        for child in &node.inner {
            walk_scoped(child, &scope, names, visit);
        }
    }
}

fn scope_component(node: &AstNode) -> Option<String> {
    let mut name = match node.name.as_deref() {
        Some(name) => normalize_name(name),
        None if node.kind == "NamespaceDecl" => "(anonymous namespace)".into(),
        None => return None,
    };
    if node.kind == "ClassTemplateSpecializationDecl" && !name.contains('<') {
        let args = node
            .inner
            .iter()
            .filter(|n| n.kind == "TemplateArgument")
            .map(|arg| {
                arg.qual_type().map(normalize_name).or_else(|| {
                    arg.value.as_ref().and_then(|v| match v {
                        serde_json::Value::String(s) => Some(s.clone()),
                        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                            Some(v.to_string())
                        }
                        _ => None,
                    })
                })
            })
            .collect::<Option<Vec<_>>>()?;
        name = format!("{name}<{}>", args.join(", "));
    }
    Some(name)
}

fn declaration_rank(node: &AstNode) -> (bool, bool, bool, usize) {
    (
        node.is_implicit != Some(true),
        node.qual_type().is_some(),
        node.inner
            .iter()
            .any(|n| matches!(n.kind.as_str(), "CompoundStmt" | "CXXTryStmt")),
        node.inner
            .iter()
            .filter(|n| n.kind == "ParmVarDecl" && n.name.is_some() && n.qual_type().is_some())
            .count(),
    )
}

pub(super) fn source_signature(ast: &AstNode, symbol: &str) -> Result<SourceSignature> {
    let mut names = BTreeMap::new();
    let mut found: Option<(&AstNode, String)> = None;
    walk_scoped(ast, "", &mut names, &mut |node, scope| {
        if node.is_function_like()
            && node.mangled_name.as_deref() == Some(symbol)
            && found
                .as_ref()
                .is_none_or(|(old, _)| declaration_rank(node) > declaration_rank(old))
        {
            found = Some((node, scope.to_owned()));
        }
    });
    let (node, scope) = found.context("target declaration missing from AST")?;
    let receiver =
        if node.kind == "CXXMethodDecl" && node.storage_class.as_deref() != Some("static") {
            Some(if let Some(id) = &node.parent_decl_context_id {
                names
                    .get(id)
                    .cloned()
                    .context("target parent context missing from AST")?
            } else {
                scope
            })
        } else {
            None
        };
    let params = node
        .inner
        .iter()
        .filter(|n| n.kind == "ParmVarDecl")
        .filter_map(|param| {
            let name = param.name.clone()?;
            let ty = param.r#type.as_ref()?;
            let spelling = ty
                .extra
                .get("desugaredQualType")
                .and_then(|v| v.as_str())
                .or(ty.qual_type.as_deref())?;
            Some((name, spelling.into()))
        })
        .collect();
    let return_type = node
        .qual_type()
        .and_then(|q| q.split_once('('))
        .map(|(r, _)| r.trim().into())
        .unwrap_or_default();
    Ok(SourceSignature {
        receiver,
        params,
        return_type,
    })
}

pub(super) fn source_object_name(ty: &str) -> String {
    let mut text = ty.trim();
    loop {
        let before = text;
        text = text.trim_end_matches(['*', '&', ' ']);
        for qualifier in ["const", "volatile", "restrict", "__restrict"] {
            if let Some(tail) = text.strip_prefix(&format!("{qualifier} ")) {
                text = tail.trim();
            }
            if let Some(head) = text.strip_suffix(qualifier) {
                if head.ends_with([' ', '*', '&']) {
                    text = head.trim();
                }
            }
        }
        if text == before {
            break;
        }
    }
    normalize_name(text)
}

pub(super) struct AbiSignature {
    pub params: Vec<String>,
    pub sret: Option<(usize, String, Option<usize>)>,
}

pub(super) fn abi_signature(ir: &str, symbol: &str) -> Result<AbiSignature> {
    let quoted = format!("@\"{symbol}\"(");
    let bare = format!("@{symbol}(");
    let line = ir
        .lines()
        .find(|line| {
            line.trim_start().starts_with("define ")
                && (line.contains(&quoted) || line.contains(&bare))
        })
        .context("target has no LLVM definition")?;
    let marker = if line.contains(&quoted) {
        &quoted
    } else {
        &bare
    };
    let tail = &line[line.find(marker).unwrap() + marker.len()..];
    let mut depth = 1;
    let mut end = None;
    let mut quoted = false;
    for (i, ch) in tail.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        }
        if quoted {
            continue;
        }
        if ch == '(' {
            depth += 1;
        }
        if ch == ')' {
            depth -= 1;
            if depth == 0 {
                end = Some(i);
                break;
            }
        }
    }
    let params = split(&tail[..end.context("unterminated LLVM signature")?]);
    let mut sret = None;
    for (i, param) in params.iter().enumerate() {
        if let Some((_, tail)) = param.split_once("sret(") {
            ensure!(sret.is_none(), "multiple sret parameters");
            let ty = tail.split_once(')').context("unterminated sret type")?.0;
            let align = param
                .split_once(" align ")
                .and_then(|(_, v)| v.split_whitespace().next())
                .map(str::parse)
                .transpose()?;
            sret = Some((
                i,
                ty.trim().trim_start_matches('%').trim_matches('"').into(),
                align,
            ));
        }
    }
    Ok(AbiSignature { params, sret })
}

fn split(text: &str) -> Vec<String> {
    let (mut depth, mut quoted, mut start) = (0usize, false, 0);
    let mut result = Vec::new();
    for (i, ch) in text.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        }
        if quoted {
            continue;
        }
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(text[start..i].trim().into());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        result.push(text[start..].trim().into());
    }
    result
}

pub(super) fn abstract_record(ast: &AstNode, name: &str) -> bool {
    let name = normalize_name(name);
    let name = name.strip_prefix("::").unwrap_or(&name);
    let (mut found, mut all_abstract) = (false, true);
    walk_scoped(ast, "", &mut BTreeMap::new(), &mut |node, scope| {
        let definition = node.extra.get("definitionData");
        if node.is_record()
            && node.is_implicit != Some(true)
            && scope == name
            && (node
                .extra
                .get("completeDefinition")
                .and_then(|v| v.as_bool())
                == Some(true)
                || definition.is_some_and(|v| v.is_object()))
        {
            found = true;
            // A conflicting complete definition disables the abstract-only
            // allocation exception. Forward/injected declarations are not votes.
            all_abstract &= definition
                .and_then(|d| d.get("isAbstract"))
                .and_then(|v| v.as_bool())
                == Some(true);
        }
    });
    found && all_abstract
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn ast(value: Value) -> AstNode {
        serde_json::from_value(value).unwrap()
    }

    fn record(name: &str, abstract_: bool) -> Value {
        json!({"kind": "CXXRecordDecl", "name": name, "completeDefinition": true,
            "definitionData": {"isAbstract": abstract_}})
    }

    #[test]
    fn abstract_records_use_exact_namespace_and_nested_scopes() {
        let root = ast(json!({"inner": [
            {"kind": "NamespaceDecl", "name": "api", "inner": [record("Face", true),
                {"kind": "CXXRecordDecl", "name": "Outer", "id": "outer", "inner": [
                    record("Face", false), {"kind": "CXXRecordDecl", "name": "Nested"}]}]},
            {"kind": "NamespaceDecl", "name": "impls", "inner": [record("Face", false)]},
            {"kind": "CXXRecordDecl", "name": "Nested", "parentDeclContextId": "outer",
                "completeDefinition": true, "definitionData": {"isAbstract": true}}
        ]}));
        assert!(abstract_record(&root, "class api::Face"));
        assert!(abstract_record(&root, "::api::Face"));
        assert!(abstract_record(&root, "api::Outer::Nested"));
        assert!(!abstract_record(&root, "Nested"));
        for name in ["Face", "impls::Face", "api::Outer::Face", "missing::Face"] {
            assert!(!abstract_record(&root, name), "matched {name}");
        }
    }

    #[test]
    fn abstract_records_ignore_forward_implicit_and_non_record_nodes() {
        let root = ast(json!({"inner": [record("Face", true),
            {"kind": "CXXRecordDecl", "name": "Face"},
            {"kind": "CXXRecordDecl", "name": "Face", "isImplicit": true,
                "completeDefinition": true, "definitionData": {}},
            {"kind": "VarDecl", "name": "Other", "definitionData": {"isAbstract": true}}
        ]}));
        assert!(abstract_record(&root, "struct Face"));
        assert!(!abstract_record(&root, "Other"));
        let forward = ast(json!({"kind": "CXXRecordDecl", "name": "F"}));
        assert!(!abstract_record(&forward, "F"));
    }

    #[test]
    fn conflicting_abstract_definitions_fail_closed_in_either_order() {
        for definitions in [
            vec![record("F", true), record("F", false)],
            vec![record("F", false), record("F", true)],
        ] {
            assert!(!abstract_record(&ast(json!({"inner": definitions})), "F"));
        }
        let root = ast(json!({"inner": [record("F", true),
            {"kind": "CXXRecordDecl", "name": "F", "completeDefinition": true}]}));
        assert!(!abstract_record(&root, "F"));
    }

    #[test]
    fn local_and_anonymous_records_cannot_masquerade_as_global_records() {
        let root = ast(json!({"inner": [
            {"kind": "FunctionDecl", "name": "f", "inner": [record("Local", true)]},
            {"kind": "NamespaceDecl", "inner": [record("Hidden", true)]}
        ]}));
        assert!(!abstract_record(&root, "Local"));
        assert!(!abstract_record(&root, "Hidden"));
        assert!(abstract_record(&root, "(anonymous namespace)::Hidden"));
    }

    #[test]
    fn template_specializations_keep_their_arguments() {
        let root = ast(json!({"kind": "NamespaceDecl", "name": "api", "inner": [
            {"kind": "ClassTemplateSpecializationDecl", "name": "Face",
                "definitionData": {"isAbstract": true}, "inner": [
                    {"kind": "TemplateArgument", "type": {"qualType": "struct Item"}}]}
        ]}));
        assert!(abstract_record(&root, "api::Face<Item>"));
        assert!(!abstract_record(&root, "api::Face"));
        assert!(!abstract_record(&root, "api::Face<Other>"));
    }

    #[test]
    fn signature_prefers_definition_over_forward_or_implicit_redeclarations() {
        let definition = json!({"kind": "FunctionDecl", "mangledName": "target",
            "type": {"qualType": "struct Result (Alias)"}, "inner": [
                {"kind": "ParmVarDecl", "name": "value", "type": {
                    "qualType": "Alias", "desugaredQualType": "const api::Value &"}},
                {"kind": "CompoundStmt"}]});
        let forward = json!({"kind": "FunctionDecl", "mangledName": "target",
            "type": {"qualType": "struct Result (Alias)"}});
        let implicit = json!({"kind": "FunctionDecl", "mangledName": "target", "isImplicit": true});
        for nodes in [
            vec![definition.clone(), forward.clone(), implicit.clone()],
            vec![implicit, forward, definition],
        ] {
            let signature = source_signature(&ast(json!({"inner": nodes})), "target").unwrap();
            assert_eq!(signature.params["value"], "const api::Value &");
            assert_eq!(signature.return_type, "struct Result");
            assert!(signature.receiver.is_none());
        }
    }

    #[test]
    fn signature_resolves_out_of_line_parent_after_the_declaration() {
        let root = ast(json!({"inner": [
            {"kind": "CXXMethodDecl", "mangledName": "target", "parentDeclContextId": "inner",
                "type": {"qualType": "int ()"}, "inner": [{"kind": "CompoundStmt"}]},
            {"kind": "NamespaceDecl", "name": "api", "inner": [
                {"kind": "CXXRecordDecl", "name": "Outer", "inner": [
                    {"kind": "CXXRecordDecl", "name": "Inner", "id": "inner", "inner": [
                        {"kind": "CXXRecordDecl", "name": "Inner", "isImplicit": true},
                        {"kind": "CXXMethodDecl", "mangledName": "target", "type": {"qualType": "int ()"}}]}]}]}
        ]}));
        let signature = source_signature(&root, "target").unwrap();
        assert_eq!(signature.receiver.as_deref(), Some("api::Outer::Inner"));
        let orphan = ast(json!({"kind": "CXXMethodDecl", "mangledName": "orphan",
            "parentDeclContextId": "missing", "type": {"qualType": "int ()"}}));
        assert!(source_signature(&orphan, "orphan").is_err());
    }

    #[test]
    fn static_methods_have_no_receiver_and_symbols_are_exact() {
        let root = ast(json!({"kind": "CXXRecordDecl", "name": "C", "inner": [
            {"kind": "CXXMethodDecl", "mangledName": "target", "storageClass": "static"},
            {"kind": "CXXMethodDecl", "mangledName": "target", "storageClass": "static",
                "type": {"qualType": "int ()"}},
            {"kind": "CXXMethodDecl", "mangledName": "target_extra", "type": {"qualType": "bool ()"}}
        ]}));
        let signature = source_signature(&root, "target").unwrap();
        assert!(signature.receiver.is_none());
        assert_eq!(signature.return_type, "int");
        assert!(source_signature(&root, "missing").is_err());
    }

    #[test]
    fn source_object_names_strip_qualifiers_and_elaborated_tags() {
        for ty in [
            "const struct api::R *const &",
            "volatile api::R &&",
            "class api::R",
        ] {
            assert_eq!(source_object_name(ty), "api::R");
        }
    }

    #[test]
    fn abi_signature_keeps_nested_attributes_and_sret_position() {
        let ir = "define void @target(ptr %this, ptr sret(%\"struct.api::R\") align 16 %r, ptr byval({ i32, i32 }) %v, i32 %n) {";
        let abi = abi_signature(ir, "target").unwrap();
        assert_eq!(abi.params.len(), 4);
        assert_eq!(abi.sret, Some((1, "struct.api::R".into(), Some(16))));
        assert!(abi_signature(ir, "target_extra").is_err());
    }
}
