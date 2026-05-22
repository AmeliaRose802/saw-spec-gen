//! ADT map builder: turns mir-json `adts` entries into a
//! `name → TypeInfo` lookup table consumed by the type parser.

use super::node::{AdtDef, MirJson};
use super::rust_types::parse_rust_type_string;
use crate::constraints::TypeInfo;
use std::collections::HashMap;

/// Build a `name → TypeInfo` table from the `adts` array. Each ADT
/// entry is dispatched to [`parse_adt_def`]; entries with an
/// unsupported `kind` are skipped silently.
pub fn build_adt_map(mir: &MirJson) -> HashMap<String, TypeInfo> {
    let mut map = HashMap::new();
    for adt in &mir.adts {
        if let Some((name, ty)) = parse_adt_def(adt) {
            map.insert(name, ty);
        }
    }
    map
}

/// Convert one [`AdtDef`] into a `(name, TypeInfo)` pair.
///
/// Recognized `kind`s:
/// - `"enum"` → [`TypeInfo::Enum`] with variant names and
///   `discriminant_bits` (default 64).
/// - `"struct"` / `"union"` → [`TypeInfo::Struct`] with field list and
///   optional `size_bytes`.
///
/// The resulting type has an empty `name` field — callers re-stamp it
/// from the lookup key (see [`super::rust_types::parse_rust_type_string`]).
pub fn parse_adt_def(adt: &AdtDef) -> Option<(String, TypeInfo)> {
    let name = adt.name.clone()?;
    let kind = adt.kind.as_deref().unwrap_or("struct");
    match kind {
        "enum" => Some((name, build_enum(adt))),
        "struct" | "union" => Some((name, build_struct(adt))),
        _ => None,
    }
}

fn build_enum(adt: &AdtDef) -> TypeInfo {
    let variant_names: Vec<String> = adt
        .variants
        .iter()
        .filter_map(|v| v.name.clone())
        .collect();
    let disc_bits = adt.discriminant_bits.unwrap_or(64) as u32;
    TypeInfo::Enum {
        name: String::new(),
        variants: variant_names,
        discriminant_bits: disc_bits,
    }
}

fn build_struct(adt: &AdtDef) -> TypeInfo {
    let empty_adt_map = HashMap::new();
    let fields: Vec<(String, TypeInfo)> = adt
        .variants
        .first()
        .map(|v| {
            v.fields
                .iter()
                .filter_map(|f| {
                    let fname = f.name.clone()?;
                    let fty_str = f.ty.as_deref().unwrap_or("()");
                    let (ty, _, _) = parse_rust_type_string(fty_str, false, &empty_adt_map);
                    Some((fname, ty))
                })
                .collect()
        })
        .unwrap_or_default();
    let size = adt.size.map(|s| s as usize);
    TypeInfo::Struct {
        name: String::new(),
        size_bytes: size,
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn from_json(v: serde_json::Value) -> MirJson {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn empty_adts_yields_empty_map() {
        let mir = from_json(json!({"adts": []}));
        assert!(build_adt_map(&mir).is_empty());
    }

    #[test]
    fn enum_adt_appears_with_variants_and_bits() {
        let mir = from_json(json!({
            "adts": [{
                "name": "E",
                "kind": "enum",
                "discriminant_bits": 32,
                "variants": [
                    {"name": "A", "fields": []},
                    {"name": "B", "fields": []}
                ]
            }]
        }));
        let map = build_adt_map(&mir);
        match map.get("E").unwrap() {
            TypeInfo::Enum { variants, discriminant_bits, .. } => {
                assert_eq!(variants, &vec!["A".to_string(), "B".to_string()]);
                assert_eq!(*discriminant_bits, 32);
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn enum_defaults_to_64_bit_discriminant() {
        let mir = from_json(json!({
            "adts": [{"name": "E", "kind": "enum", "variants": []}]
        }));
        let map = build_adt_map(&mir);
        match map.get("E").unwrap() {
            TypeInfo::Enum { discriminant_bits, .. } => assert_eq!(*discriminant_bits, 64),
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn struct_adt_collects_fields_and_size() {
        let mir = from_json(json!({
            "adts": [{
                "name": "S",
                "kind": "struct",
                "size": 16,
                "variants": [{
                    "name": "S",
                    "fields": [
                        {"name": "a", "ty": "u32"},
                        {"name": "b", "ty": "u64"}
                    ]
                }]
            }]
        }));
        let map = build_adt_map(&mir);
        match map.get("S").unwrap() {
            TypeInfo::Struct { size_bytes, fields, .. } => {
                assert_eq!(*size_bytes, Some(16));
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "a");
                assert_eq!(fields[1].1, TypeInfo::UnsignedInt(64));
            }
            other => panic!("expected Struct, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_is_dropped() {
        let mir = from_json(json!({
            "adts": [{"name": "X", "kind": "tagged_union", "variants": []}]
        }));
        assert!(build_adt_map(&mir).is_empty());
    }

    #[test]
    fn struct_default_kind_is_struct() {
        // Missing `kind` defaults to "struct".
        let mir = from_json(json!({
            "adts": [{"name": "T", "variants": [{"name": "T", "fields": []}]}]
        }));
        let map = build_adt_map(&mir);
        assert!(matches!(map.get("T"), Some(TypeInfo::Struct { .. })));
    }
}
