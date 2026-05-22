//! Loading and parsing of clang JSON AST dumps into [`AstNode`].
//!
//! Handles three input shapes:
//!   1. The standard single-object `clang -ast-dump=json` output.
//!   2. The concatenated multi-object output produced when
//!      `-ast-dump-filter=Name` matches more than one decl.
//!   3. A synthetic merge of multiple parsed ASTs (one per header).

use super::node::AstNode;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Maximum input file size that [`parse_ast`] will load (100 MB).
///
/// Clang AST dumps for large translation units routinely exceed this and
/// would OOM the host. The error message guides users toward
/// `-ast-dump-filter` or pre-filtering with `jq`.
pub const MAX_AST_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Read a clang JSON AST dump from disk and decode it into an [`AstNode`].
///
/// Enforces [`MAX_AST_FILE_SIZE`] and accepts both single-object dumps
/// and concatenated multi-object dumps (from `-ast-dump-filter`).
pub fn parse_ast(path: &Path) -> Result<AstNode> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_AST_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "AST file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use clang -ast-dump-filter=<function> to dump only specific declarations\n\
             \n  2. Split the translation unit into smaller files\n\
             \n  3. Pre-filter with: jq 'select(.name == \"MyFunc\")' large_ast.json > filtered.json",
            path.display(),
            size_mb,
            MAX_AST_FILE_SIZE / (1024 * 1024),
        );
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_ast_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

/// Parse a JSON string into an [`AstNode`].  Tries a single-object decode
/// first and falls back to the concatenated-objects splitter so callers
/// can transparently feed in `-ast-dump-filter` output.
pub fn parse_ast_str(content: &str) -> Result<AstNode> {
    if let Ok(node) = serde_json::from_str::<AstNode>(content) {
        return Ok(node);
    }
    let objects = split_concatenated_json(content)?;
    if objects.is_empty() {
        anyhow::bail!("No JSON objects found in input");
    }
    if objects.len() == 1 {
        let v = objects.into_iter().next().unwrap();
        return Ok(serde_json::from_value(v)?);
    }
    let synthetic = Value::Object(
        [
            ("kind".to_string(), Value::String("TranslationUnitDecl".into())),
            ("inner".to_string(), Value::Array(objects)),
        ]
        .into_iter()
        .collect(),
    );
    Ok(serde_json::from_value(synthetic)?)
}

/// Split a string containing one or more top-level JSON objects pasted
/// back-to-back. Handles nested braces inside string literals.
pub fn split_concatenated_json(content: &str) -> Result<Vec<Value>> {
    let mut objects = Vec::new();
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, c) in content.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let slice = &content[s..=i];
                        let v: Value = serde_json::from_str(slice).with_context(|| {
                            format!("Failed to parse JSON object at byte offset {s}")
                        })?;
                        objects.push(v);
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }

    Ok(objects)
}

/// Merge multiple parsed ASTs into a single synthetic `TranslationUnitDecl`.
///
/// Only flattens inputs that are themselves `TranslationUnitDecl`s; any
/// other kind (e.g. a filtered `CXXRecordDecl` dump) is appended as a
/// direct child of the synthetic root. This preserves enclosing-class
/// context that would otherwise be lost — see the original module
/// docstring for the gory details about `-ast-dump-filter` shape quirks.
pub fn merge_asts(asts: Vec<AstNode>) -> AstNode {
    let mut combined_inner: Vec<AstNode> = Vec::new();
    for ast in asts {
        if ast.kind == "TranslationUnitDecl" {
            combined_inner.extend(ast.inner);
        } else {
            combined_inner.push(ast);
        }
    }
    AstNode {
        kind: "TranslationUnitDecl".into(),
        inner: combined_inner,
        ..AstNode::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_single_object() {
        let content = r#"{"kind":"TranslationUnitDecl","inner":[]}"#;
        let ast = parse_ast_str(content).unwrap();
        assert_eq!(ast.kind, "TranslationUnitDecl");
    }

    #[test]
    fn parses_concatenated_objects_into_synthetic_tu() {
        let content = r#"{"kind":"FunctionDecl","name":"foo","type":{"qualType":"void ()"}}
{"kind":"FunctionDecl","name":"bar","type":{"qualType":"int (int)"}}"#;
        let ast = parse_ast_str(content).unwrap();
        assert_eq!(ast.kind, "TranslationUnitDecl");
        assert_eq!(ast.inner.len(), 2);
        assert_eq!(ast.inner[0].name.as_deref(), Some("foo"));
        assert_eq!(ast.inner[1].name.as_deref(), Some("bar"));
    }

    #[test]
    fn handles_nested_braces_in_string_values() {
        let content = r#"{"kind":"A","note":"has { braces }","inner":[]}
{"kind":"B","inner":[]}"#;
        let ast = parse_ast_str(content).unwrap();
        assert_eq!(ast.kind, "TranslationUnitDecl");
        assert_eq!(ast.inner.len(), 2);
    }

    #[test]
    fn empty_input_fails() {
        assert!(parse_ast_str("").is_err());
    }

    #[test]
    fn small_file_round_trip() {
        let dir = std::env::temp_dir().join("saw_spec_gen_parse_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("small.json");
        std::fs::write(&path, r#"{"kind":"TranslationUnitDecl","inner":[]}"#).unwrap();
        let ast = parse_ast(&path).unwrap();
        assert_eq!(ast.kind, "TranslationUnitDecl");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors_with_stat_message() {
        let path = std::env::temp_dir().join("definitely_not_here_xyzzy.json");
        let err = parse_ast(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to stat") || msg.contains("Failed to read"));
    }

    #[test]
    fn max_file_size_constant_is_100mb() {
        assert_eq!(MAX_AST_FILE_SIZE, 100 * 1024 * 1024);
    }

    #[test]
    fn merge_flattens_tu_children_but_keeps_filtered_records() {
        let tu: AstNode = serde_json::from_value(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{"kind": "FunctionDecl", "name": "a"}]
        }))
        .unwrap();
        let filtered: AstNode = serde_json::from_value(json!({
            "kind": "CXXRecordDecl",
            "name": "IFace",
            "inner": [{"kind": "CXXMethodDecl", "name": "m", "virtual": true}]
        }))
        .unwrap();
        let merged = merge_asts(vec![tu, filtered]);
        assert_eq!(merged.kind, "TranslationUnitDecl");
        assert_eq!(merged.inner.len(), 2);
        assert_eq!(merged.inner[0].kind, "FunctionDecl");
        assert_eq!(merged.inner[1].kind, "CXXRecordDecl");
    }
}
