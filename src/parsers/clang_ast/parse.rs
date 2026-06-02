//! Loading and parsing of clang JSON AST dumps into [`AstNode`].
//!
//! Handles three input shapes:
//!   1. The standard single-object `clang -ast-dump=json` output.
//!   2. The concatenated multi-object output produced when
//!      `-ast-dump-filter=Name` matches more than one decl.
//!   3. A synthetic merge of multiple parsed ASTs (one per header).
//!
//! Both file and string inputs flow through
//! [`serde_json::Deserializer`]'s streaming reader API, which decodes
//! one top-level JSON value at a time without materialising the full
//! source text in memory. This keeps peak memory roughly proportional
//! to the *parsed* AST size rather than `2 ×` the file size that the
//! previous `read_to_string` + `from_str` pipeline required, and lets
//! the on-disk size guard be a sanity ceiling rather than a hard cap.

use super::node::AstNode;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Sanity ceiling on input AST file size (2 GiB).
///
/// Streamed parsing keeps peak memory proportional to the parsed AST
/// shape rather than the on-disk byte length, so this limit is now a
/// guardrail against pathological inputs (e.g. a clang log accidentally
/// fed in as JSON) rather than a memory cap. Raise via
/// `SAW_SPEC_GEN_MAX_AST_BYTES` when verifying very large translation
/// units.
pub const MAX_AST_FILE_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Environment override for [`MAX_AST_FILE_SIZE`] (raw byte count).
const MAX_AST_FILE_SIZE_ENV: &str = "SAW_SPEC_GEN_MAX_AST_BYTES";

/// Buffer size for the streaming file reader (1 MiB).
const READ_BUFFER_BYTES: usize = 1024 * 1024;

fn effective_max_ast_size() -> u64 {
    std::env::var(MAX_AST_FILE_SIZE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(MAX_AST_FILE_SIZE)
}

/// Read a clang JSON AST dump from disk and decode it into an [`AstNode`].
///
/// Uses a streamed `BufReader` so the raw JSON text is never
/// materialised as a single `String`; the size guard is enforced via
/// `metadata()` before any data is read. Accepts both single-object
/// dumps and concatenated multi-object dumps (from `-ast-dump-filter`).
pub fn parse_ast(path: &Path) -> Result<AstNode> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    let max_size = effective_max_ast_size();
    if file_size > max_size {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "AST file {} is too large ({} MB, sanity limit is {} MB). \
             Streaming reduces peak memory but cannot bound the parsed \
             AST itself — if you really need to load this file, raise \
             the limit via {}=<bytes>. Otherwise, try:\n\
             \n  1. Use clang -ast-dump-filter=<function> to dump only specific declarations\n\
             \n  2. Split the translation unit into smaller files\n\
             \n  3. Pre-filter with: jq 'select(.name == \"MyFunc\")' large_ast.json > filtered.json",
            path.display(),
            size_mb,
            max_size / (1024 * 1024),
            MAX_AST_FILE_SIZE_ENV,
        );
    }

    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    parse_ast_from_reader(reader)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

/// Parse one or more top-level AST objects out of an arbitrary
/// [`Read`] source, streaming the bytes through serde_json without an
/// intermediate `String`.
pub fn parse_ast_from_reader<R: Read>(reader: R) -> Result<AstNode> {
    let stream = serde_json::Deserializer::from_reader(reader).into_iter::<AstNode>();
    let mut nodes: Vec<AstNode> = Vec::new();
    for item in stream {
        nodes.push(item.context("Failed to deserialize AST node")?);
    }
    finalize_ast_nodes(nodes)
}

/// Parse a JSON string into an [`AstNode`]. Streams through
/// `Deserializer::from_str` so single-object and concatenated-object
/// inputs share a single code path with the file-based loader.
///
/// Kept on the public surface for tests and embedders even though the
/// in-tree `parse_ast` no longer routes through it.
#[allow(dead_code)]
pub fn parse_ast_str(content: &str) -> Result<AstNode> {
    let stream = serde_json::Deserializer::from_str(content).into_iter::<AstNode>();
    let mut nodes: Vec<AstNode> = Vec::new();
    for item in stream {
        nodes.push(item.context("Failed to deserialize AST node")?);
    }
    finalize_ast_nodes(nodes)
}

/// Collapse the per-object stream into a single [`AstNode`]: pass a
/// solo object through verbatim, or wrap multiple objects in a
/// synthetic `TranslationUnitDecl` so downstream walkers always see a
/// normal root.
fn finalize_ast_nodes(mut nodes: Vec<AstNode>) -> Result<AstNode> {
    if nodes.is_empty() {
        anyhow::bail!("No JSON objects found in input");
    }
    if nodes.len() == 1 {
        return Ok(nodes.pop().unwrap());
    }
    Ok(AstNode {
        kind: "TranslationUnitDecl".into(),
        inner: nodes,
        ..AstNode::default()
    })
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
    fn streaming_reader_matches_string_path() {
        let content = r#"{"kind":"FunctionDecl","name":"foo"}
{"kind":"FunctionDecl","name":"bar"}"#;
        let from_str = parse_ast_str(content).unwrap();
        let from_rdr = parse_ast_from_reader(content.as_bytes()).unwrap();
        assert_eq!(from_str.kind, from_rdr.kind);
        assert_eq!(from_str.inner.len(), from_rdr.inner.len());
        assert_eq!(
            from_str.inner[0].name.as_deref(),
            from_rdr.inner[0].name.as_deref()
        );
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
        assert!(
            msg.contains("Failed to stat")
                || msg.contains("Failed to read")
                || msg.contains("Failed to open")
        );
    }

    #[test]
    fn max_file_size_constant_is_2gib() {
        assert_eq!(MAX_AST_FILE_SIZE, 2 * 1024 * 1024 * 1024);
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
