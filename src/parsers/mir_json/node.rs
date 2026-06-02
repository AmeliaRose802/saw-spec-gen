//! Typed Rust view of `mir-json` output.
//!
//! mir-json serializes Rust MIR to JSON, preserving full type
//! information including enum variants, reference mutability,
//! lifetimes, etc. This module provides serde structs that decode the
//! shape we actually consume — anything we don't model is preserved
//! via `serde(flatten)` for round-tripping.

use serde::Deserialize;
use serde_json::{Map, Value};

/// Top-level mir-json document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MirJson {
    #[serde(default)]
    pub fns: Vec<FnDef>,
    #[serde(default)]
    pub adts: Vec<AdtDef>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// A function or method declaration emitted by mir-json.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FnDef {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub args: Vec<ArgDef>,
    #[serde(default, rename = "return_ty")]
    pub return_ty: Option<Value>,
    /// Explicit async / coroutine marker. mir-json sometimes sets this
    /// on the function; we also sniff the `return_ty` string for
    /// coroutine signatures (`impl Future`, `Poll`, `GenFuture`,
    /// `Coroutine`).
    #[serde(default)]
    pub is_async: Option<bool>,
    /// Free-form attribute strings (`"nounwind"`, `"noexcept"`, …).
    #[serde(default)]
    pub attrs: Vec<Value>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// One argument of a [`FnDef`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArgDef {
    #[serde(default)]
    pub name: Option<String>,
    /// Type string emitted by mir-json. We treat this as opaque text
    /// and parse it in [`super::rust_types`].
    #[serde(default)]
    pub ty: Option<String>,
    /// Mutability wrapper: `{"kind": "Mut"}` or `{"kind": "Not"}`.
    #[serde(default, rename = "mut")]
    pub mutability: Option<MutKind>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MutKind {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

impl MutKind {
    pub fn is_mut(&self) -> bool {
        self.kind.as_deref() == Some("Mut")
    }
}

/// An algebraic data type definition — enum, struct, or union.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdtDef {
    #[serde(default)]
    pub name: Option<String>,
    /// `"enum"`, `"struct"`, or `"union"` (defaults to `"struct"` when
    /// missing).
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub variants: Vec<AdtVariant>,
    #[serde(default)]
    pub discriminant_bits: Option<u64>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdtVariant {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fields: Vec<AdtField>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdtField {
    #[serde(default)]
    pub name: Option<String>,
    /// Field type. mir-json uses an unquoted Rust-type string here,
    /// which we re-feed through the type parser in
    /// [`super::rust_types`].
    #[serde(default)]
    pub ty: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_empty_document() {
        let v = json!({});
        let m: MirJson = serde_json::from_value(v).unwrap();
        assert!(m.fns.is_empty());
        assert!(m.adts.is_empty());
    }

    #[test]
    fn parses_simple_function() {
        let v = json!({
            "fns": [{
                "name": "add",
                "args": [
                    {"name": "a", "ty": "ty::Int::I32", "mut": {"kind": "Not"}},
                    {"name": "b", "ty": "ty::Int::I32", "mut": {"kind": "Mut"}}
                ],
                "return_ty": "ty::Int::I32"
            }]
        });
        let m: MirJson = serde_json::from_value(v).unwrap();
        assert_eq!(m.fns.len(), 1);
        let f = &m.fns[0];
        assert_eq!(f.name.as_deref(), Some("add"));
        assert_eq!(f.args.len(), 2);
        assert_eq!(f.args[0].ty.as_deref(), Some("ty::Int::I32"));
        assert!(!f.args[0].mutability.as_ref().unwrap().is_mut());
        assert!(f.args[1].mutability.as_ref().unwrap().is_mut());
    }

    #[test]
    fn parses_async_marker() {
        let v = json!({
            "fns": [{"name": "f", "is_async": true, "return_ty": "impl Future"}]
        });
        let m: MirJson = serde_json::from_value(v).unwrap();
        assert_eq!(m.fns[0].is_async, Some(true));
    }

    #[test]
    fn parses_enum_adt() {
        let v = json!({
            "adts": [{
                "name": "LatchState",
                "kind": "enum",
                "discriminant_bits": 64,
                "variants": [
                    {"name": "Unlatched", "fields": []},
                    {"name": "Latched", "fields": []}
                ]
            }]
        });
        let m: MirJson = serde_json::from_value(v).unwrap();
        let a = &m.adts[0];
        assert_eq!(a.kind.as_deref(), Some("enum"));
        assert_eq!(a.discriminant_bits, Some(64));
        assert_eq!(a.variants.len(), 2);
        assert_eq!(a.variants[0].name.as_deref(), Some("Unlatched"));
    }

    #[test]
    fn parses_struct_adt_with_fields() {
        let v = json!({
            "adts": [{
                "name": "Header",
                "kind": "struct",
                "size": 24,
                "variants": [{
                    "name": "Header",
                    "fields": [
                        {"name": "version", "ty": "ty::Uint::U32"},
                        {"name": "length", "ty": "ty::Uint::U64"}
                    ]
                }]
            }]
        });
        let m: MirJson = serde_json::from_value(v).unwrap();
        let a = &m.adts[0];
        assert_eq!(a.size, Some(24));
        assert_eq!(a.variants[0].fields.len(), 2);
        assert_eq!(a.variants[0].fields[0].name.as_deref(), Some("version"));
        assert_eq!(a.variants[0].fields[0].ty.as_deref(), Some("ty::Uint::U32"));
    }

    #[test]
    fn unknown_fields_round_trip_through_extra() {
        let v = json!({
            "fns": [{"name": "f", "future_field": 42}]
        });
        let m: MirJson = serde_json::from_value(v).unwrap();
        assert!(m.fns[0].extra.contains_key("future_field"));
    }
}
