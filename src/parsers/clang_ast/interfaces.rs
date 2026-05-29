//! Detection of interface types referenced by class fields but missing
//! from the AST. Used to surface stub-generation needs for headers the
//! consumer never `#include`d.

use super::node::AstNode;
use super::visitor::{walk, ClassStack, Visitor, WalkAction};
use std::collections::HashSet;

/// A reference to an interface type that is missing from the AST.
#[derive(Debug, Clone)]
pub struct MissingInterfaceRef {
    /// Class that holds the field (e.g. `"KeyExchangeSession"`).
    pub owning_class: String,
    /// Field name (e.g. `"KeyStore"`).
    pub field_name: String,
    /// Wrapper type description (e.g. `"std::unique_ptr"` or `"raw pointer"`).
    pub wrapper: String,
    /// Bare interface name (e.g. `"IKeyStore"`).
    pub interface_name: String,
}

/// Detect interface types referenced by class fields but missing from
/// the AST. Recognizes `unique_ptr<IFoo>`, `shared_ptr<IFoo>`, and
/// `IFoo*` (any name matching the C++ `IUppercase…` convention).
pub fn detect_missing_interfaces(ast: &AstNode) -> Vec<MissingInterfaceRef> {
    let mut known: HashSet<String> = HashSet::new();
    let mut kv = KnownClassesVisitor { out: &mut known };
    walk(ast, &mut kv);

    let mut missing: Vec<MissingInterfaceRef> = Vec::new();
    let mut mv = FieldVisitor {
        out: &mut missing,
        known: &known,
        stack: ClassStack::new(),
    };
    walk(ast, &mut mv);

    missing.sort_by(|a, b| a.interface_name.cmp(&b.interface_name));
    missing
        .dedup_by(|a, b| a.interface_name == b.interface_name && a.owning_class == b.owning_class);
    missing
}

struct KnownClassesVisitor<'a> {
    out: &'a mut HashSet<String>,
}

impl<'a> Visitor for KnownClassesVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if matches!(
            node.kind.as_str(),
            "CXXRecordDecl" | "ClassTemplateSpecializationDecl"
        ) {
            if let Some(name) = node.name.as_deref() {
                self.out.insert(name.to_string());
            }
        }
        WalkAction::Continue
    }
}

struct FieldVisitor<'a> {
    out: &'a mut Vec<MissingInterfaceRef>,
    known: &'a HashSet<String>,
    stack: ClassStack,
}

impl<'a> Visitor for FieldVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if node.kind == "FieldDecl" {
            if let Some(owning) = self.stack.current() {
                if let Some(qt) = node.qual_type() {
                    let field_name = node.name.as_deref().unwrap_or("unnamed");
                    if let Some((wrapper, iface)) = extract_interface_from_type(qt) {
                        if !self.known.contains(&iface) {
                            self.out.push(MissingInterfaceRef {
                                owning_class: owning.to_string(),
                                field_name: field_name.to_string(),
                                wrapper,
                                interface_name: iface,
                            });
                        }
                    }
                }
            }
        }
        self.stack.push_if_class(node);
        WalkAction::Continue
    }
    fn leave(&mut self, node: &AstNode) {
        self.stack.pop_if(node.class_name().is_some());
    }
}

/// Recognize `std::unique_ptr<T>`, `std::shared_ptr<T>`, and `T*` shapes
/// where `T` looks like a C++ interface (`I` + uppercase).
pub fn extract_interface_from_type(qual_type: &str) -> Option<(String, String)> {
    let t = qual_type.trim();
    for wrapper in ["unique_ptr", "shared_ptr"] {
        if let Some(pos) = t.find(wrapper) {
            let after = &t[pos + wrapper.len()..];
            if let Some(start) = after.find('<') {
                let inner = &after[start + 1..];
                if let Some(end) = inner.find('>') {
                    let inner_ty = inner[..end].trim();
                    let clean = inner_ty
                        .split_whitespace()
                        .next()
                        .unwrap_or(inner_ty)
                        .trim_end_matches('*');
                    if looks_like_interface(clean) {
                        return Some((format!("std::{wrapper}"), clean.to_string()));
                    }
                }
            }
        }
    }
    if t.ends_with('*') {
        let pointee = t.trim_end_matches('*').trim();
        let clean = pointee
            .trim_start_matches("const ")
            .trim_start_matches("struct ")
            .trim_start_matches("class ")
            .split_whitespace()
            .next()
            .unwrap_or(pointee);
        if looks_like_interface(clean) {
            return Some(("raw pointer".to_string(), clean.to_string()));
        }
    }
    None
}

/// True for `IFollowedByUppercase…` (C++ interface convention).
pub fn looks_like_interface(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    chars.len() >= 2 && chars[0] == 'I' && chars[1].is_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn interface_naming_heuristic() {
        assert!(looks_like_interface("IFoo"));
        assert!(looks_like_interface("IKeyStore"));
        assert!(!looks_like_interface("Init"));
        assert!(!looks_like_interface("i"));
        assert!(!looks_like_interface("ICamelCase".to_lowercase().as_str()));
    }

    #[test]
    fn extracts_unique_ptr_target() {
        assert_eq!(
            extract_interface_from_type("std::unique_ptr<IKeyStore>"),
            Some(("std::unique_ptr".into(), "IKeyStore".into()))
        );
    }

    #[test]
    fn extracts_shared_ptr_target() {
        assert_eq!(
            extract_interface_from_type("std::shared_ptr<IFoo>"),
            Some(("std::shared_ptr".into(), "IFoo".into()))
        );
    }

    #[test]
    fn extracts_raw_pointer() {
        assert_eq!(
            extract_interface_from_type("const IKeyStore *"),
            Some(("raw pointer".into(), "IKeyStore".into()))
        );
    }

    #[test]
    fn ignores_non_interface_names() {
        assert!(extract_interface_from_type("std::unique_ptr<Foo>").is_none());
        assert!(extract_interface_from_type("int *").is_none());
    }

    #[test]
    fn detects_only_unknown_interfaces() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "CXXRecordDecl", "name": "IKnown"},
                {"kind": "CXXRecordDecl", "name": "Owner", "inner": [
                    {"kind": "FieldDecl", "name": "known",
                     "type": {"qualType": "std::unique_ptr<IKnown>"}},
                    {"kind": "FieldDecl", "name": "unknown",
                     "type": {"qualType": "std::unique_ptr<IMissing>"}}
                ]}
            ]
        }));
        let m = detect_missing_interfaces(&ast);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].interface_name, "IMissing");
        assert_eq!(m[0].owning_class, "Owner");
    }
}
