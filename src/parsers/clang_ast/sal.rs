//! SAL annotation parsing for `AnnotateAttr` nodes.
//!
//! Clang's JSON dumper (since clang 17) keeps the `AnnotateAttr` kind
//! but drops the string argument that the text dumper preserves. We
//! recover the macro name by reading the source file at the attribute's
//! `expansionLoc` byte range — see [`super::source_cache`].

use super::node::AstNode;
use super::source_cache::read_source_macro_with_args;
use crate::constraints::Annotation;

/// Parse one `AnnotateAttr` child node into a SAL [`Annotation`].
/// Returns `None` for non-`AnnotateAttr` kinds.
pub fn parse_sal_annotation(attr: &AstNode) -> Option<Annotation> {
    if attr.kind != "AnnotateAttr" {
        return None;
    }
    let value_owned: String;
    let value: &str = if let Some(v) = attr.value.as_ref().and_then(|v| v.as_str()) {
        v
    } else {
        let exp = attr.range.as_ref()?.begin.as_ref()?.expansion_loc.as_ref()?;
        let file = exp.file.as_deref()?;
        let offset = exp.offset? as usize;
        let tok_len = exp.tok_len? as usize;
        // `tok_len` covers only the macro name. For sized variants
        // like `_In_reads_(8)` we also need the `(8)` portion, which
        // the helper reads from the source if it directly follows.
        value_owned = read_source_macro_with_args(file, offset, tok_len)?;
        &value_owned
    };
    classify(value)
}

fn classify(value: &str) -> Option<Annotation> {
    let inner_count = |prefix: &str| -> Option<usize> {
        value
            .strip_prefix(prefix)?
            .strip_suffix(')')?
            .parse::<usize>()
            .ok()
    };
    if let Some(n) = inner_count("_In_reads_(") {
        return Some(Annotation::InReads(n));
    }
    if let Some(n) = inner_count("_In_reads_bytes_(") {
        return Some(Annotation::InReads(n));
    }
    if let Some(n) = inner_count("_Out_writes_(") {
        return Some(Annotation::OutWrites(n));
    }
    if let Some(n) = inner_count("_Out_writes_bytes_(") {
        return Some(Annotation::OutWrites(n));
    }
    match value {
        "_In_" | "_In_opt_" => Some(Annotation::InReads(0)),
        "_Out_" | "_Out_opt_" => Some(Annotation::OutWrites(0)),
        "_Inout_" | "_Inout_opt_" => Some(Annotation::Inout),
        "_Pre_valid_" => Some(Annotation::PreValid),
        "_Post_invalid_" => Some(Annotation::PostInvalid),
        "_Ret_maybenull_" => Some(Annotation::Custom("_Ret_maybenull_".into())),
        "_Must_inspect_result_" => Some(Annotation::MustInspectResult),
        "__declspec(nothrow)" | "nothrow" => Some(Annotation::NoThrow),
        _ => Some(Annotation::Custom(value.into())),
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
    fn parses_in_reads_with_count() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_In_reads_(16)"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::InReads(16)));
    }

    #[test]
    fn parses_in_reads_bytes() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_In_reads_bytes_(256)"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::InReads(256)));
    }

    #[test]
    fn parses_out_writes() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_Out_writes_(32)"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::OutWrites(32)));
    }

    #[test]
    fn parses_bare_in_and_out() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_In_"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::InReads(0)));
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_Out_"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::OutWrites(0)));
    }

    #[test]
    fn parses_inout_variants() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_Inout_"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::Inout));
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_Inout_opt_"}));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::Inout));
    }

    #[test]
    fn ignores_non_annotate_attrs() {
        let a = parse(json!({"kind": "OverrideAttr"}));
        assert!(parse_sal_annotation(&a).is_none());
    }

    #[test]
    fn unknown_value_becomes_custom() {
        let a = parse(json!({"kind": "AnnotateAttr", "value": "_Some_New_Macro_"}));
        match parse_sal_annotation(&a) {
            Some(Annotation::Custom(s)) => assert_eq!(s, "_Some_New_Macro_"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_source_token_when_value_missing() {
        let dir = std::env::temp_dir().join("saw_spec_gen_sal_fallback");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hdr.h");
        std::fs::write(&path, b"void f(_In_ int x);").unwrap();
        let a = parse(json!({
            "kind": "AnnotateAttr",
            "range": {
                "begin": {
                    "expansionLoc": {
                        "file": path.to_string_lossy(),
                        "offset": 7,
                        "tokLen": 4
                    }
                }
            }
        }));
        assert_eq!(parse_sal_annotation(&a), Some(Annotation::InReads(0)));
        let _ = std::fs::remove_file(&path);
    }
}
