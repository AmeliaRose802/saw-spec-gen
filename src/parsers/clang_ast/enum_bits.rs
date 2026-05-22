//! Whole-AST enum-width collection.
//!
//! Used by the post-processing pass to fall back unresolved
//! `llvm_alias "EnumName"` references to `llvm_int N` regardless of
//! whether any collected function actually references the enum.

use super::node::AstNode;
use super::type_ctx::enum_bits_from_underlying;
use super::visitor::{walk, Visitor, WalkAction};
use std::collections::HashMap;

/// Walk the entire AST and return every `EnumDecl` as a
/// `name → bit_width` map. When the same enum appears with multiple
/// underlying-type entries (e.g. duplicate forward declarations), the
/// widest one wins.
pub fn collect_all_enum_bits(ast: &AstNode) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    let mut v = EnumBitsVisitor { out: &mut out };
    walk(ast, &mut v);
    out
}

struct EnumBitsVisitor<'a> {
    out: &'a mut HashMap<String, u32>,
}

impl<'a> Visitor for EnumBitsVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if node.kind != "EnumDecl" {
            return WalkAction::Continue;
        }
        if let Some(name) = node.name.as_deref() {
            let bits = node
                .fixed_underlying_type
                .as_ref()
                .and_then(|t| t.qual_type.as_deref())
                .map(enum_bits_from_underlying)
                .unwrap_or(32);
            let entry = self.out.entry(name.to_string()).or_insert(bits);
            if bits > *entry {
                *entry = bits;
            }
        }
        WalkAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_enums_with_default_width() {
        let ast: AstNode = serde_json::from_value(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "EnumDecl", "name": "Plain", "inner": [
                    {"kind": "EnumConstantDecl", "name": "A"}
                ]}
            ]
        })).unwrap();
        let m = collect_all_enum_bits(&ast);
        assert_eq!(m.get("Plain"), Some(&32));
    }

    #[test]
    fn widest_underlying_wins_on_duplicates() {
        let ast: AstNode = serde_json::from_value(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "EnumDecl", "name": "E",
                 "fixedUnderlyingType": {"qualType": "uint8_t"}},
                {"kind": "EnumDecl", "name": "E",
                 "fixedUnderlyingType": {"qualType": "uint64_t"}}
            ]
        })).unwrap();
        let m = collect_all_enum_bits(&ast);
        assert_eq!(m.get("E"), Some(&64));
    }
}
