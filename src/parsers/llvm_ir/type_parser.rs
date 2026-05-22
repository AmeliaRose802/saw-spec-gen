//! Convert LLVM IR type strings into [`TypeInfo`] values.

use super::struct_types::{compute_struct_size, parse_array_shape, IrStructDef};
use crate::constraints::TypeInfo;
use std::collections::HashMap;

/// Parse an LLVM IR type string into a [`TypeInfo`].
///
/// Handles scalar primitives, byte / non-byte arrays, opaque types,
/// and named struct references (which are resolved against
/// `struct_map`). A trailing `*` is stripped because callers wrap the
/// result with [`TypeInfo::Pointer`] when appropriate — duplicating
/// that work here would double-wrap pointer types.
pub fn parse_ir_type_resolved(
    type_str: &str,
    struct_map: &HashMap<String, IrStructDef>,
) -> TypeInfo {
    let t = type_str.trim().trim_end_matches('*');
    match t {
        "void" => TypeInfo::Void,
        "i1" => TypeInfo::Bool,
        "i8" => TypeInfo::SignedInt(8),
        "i16" => TypeInfo::SignedInt(16),
        "i32" => TypeInfo::SignedInt(32),
        "i64" => TypeInfo::SignedInt(64),
        "i128" => TypeInfo::SignedInt(128),
        "ptr" => TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
        t if t.starts_with('[') => parse_array(t, struct_map),
        t if t.starts_with('%') => parse_named_struct(t, struct_map),
        _ => opaque(t),
    }
}

fn parse_array(t: &str, struct_map: &HashMap<String, IrStructDef>) -> TypeInfo {
    if let Some((count, inner_str)) = parse_array_shape(t) {
        let inner = parse_ir_type_resolved(inner_str, struct_map);
        if matches!(inner, TypeInfo::SignedInt(8) | TypeInfo::UnsignedInt(8)) {
            return TypeInfo::ByteArray(count);
        }
    }
    opaque(t)
}

fn parse_named_struct(t: &str, struct_map: &HashMap<String, IrStructDef>) -> TypeInfo {
    let display_name = t.trim_start_matches('%').trim_matches('"');
    if let Some(def) = struct_map.get(t) {
        let size = compute_struct_size(def, struct_map);
        let fields: Vec<(String, TypeInfo)> = def
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| (format!("field{i}"), parse_ir_type_resolved(f, struct_map)))
            .collect();
        TypeInfo::Struct {
            name: display_name.into(),
            size_bytes: size,
            fields,
        }
    } else {
        opaque(display_name)
    }
}

fn opaque(name: &str) -> TypeInfo {
    TypeInfo::Opaque {
        name: name.into(),
        size_bytes: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalar_primitives() {
        let m = HashMap::new();
        assert_eq!(parse_ir_type_resolved("void", &m), TypeInfo::Void);
        assert_eq!(parse_ir_type_resolved("i1", &m), TypeInfo::Bool);
        assert_eq!(parse_ir_type_resolved("i32", &m), TypeInfo::SignedInt(32));
        assert_eq!(parse_ir_type_resolved("i64", &m), TypeInfo::SignedInt(64));
    }

    #[test]
    fn trailing_star_is_stripped() {
        // Caller wraps in Pointer; type_resolved returns the pointee.
        let m = HashMap::new();
        assert_eq!(parse_ir_type_resolved("i32*", &m), TypeInfo::SignedInt(32));
    }

    #[test]
    fn byte_array_becomes_byte_array_variant() {
        let m = HashMap::new();
        assert_eq!(parse_ir_type_resolved("[16 x i8]", &m), TypeInfo::ByteArray(16));
    }

    #[test]
    fn non_byte_array_falls_through_to_opaque() {
        let m = HashMap::new();
        match parse_ir_type_resolved("[4 x i32]", &m) {
            TypeInfo::Opaque { name, .. } => assert_eq!(name, "[4 x i32]"),
            other => panic!("expected Opaque, got {other:?}"),
        }
    }

    #[test]
    fn named_struct_without_def_is_opaque() {
        let m = HashMap::new();
        match parse_ir_type_resolved("%struct.MyData", &m) {
            TypeInfo::Opaque { name, .. } => assert_eq!(name, "struct.MyData"),
            other => panic!("expected Opaque, got {other:?}"),
        }
    }

    #[test]
    fn named_struct_with_def_resolves_fields_and_size() {
        let mut m = HashMap::new();
        m.insert(
            "%struct.Pair".into(),
            IrStructDef {
                fields: vec!["i32".into(), "i32".into()],
                is_packed: false,
            },
        );
        match parse_ir_type_resolved("%struct.Pair", &m) {
            TypeInfo::Struct { name, size_bytes, fields } => {
                assert_eq!(name, "struct.Pair");
                assert_eq!(size_bytes, Some(8));
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "field0");
            }
            other => panic!("expected Struct, got {other:?}"),
        }
    }

    #[test]
    fn quoted_struct_name_is_unquoted_in_display_name() {
        let mut m = HashMap::new();
        m.insert(
            r#"%"struct.foo::Bar""#.into(),
            IrStructDef {
                fields: vec!["i32".into()],
                is_packed: false,
            },
        );
        match parse_ir_type_resolved(r#"%"struct.foo::Bar""#, &m) {
            TypeInfo::Struct { name, .. } => assert_eq!(name, "struct.foo::Bar"),
            other => panic!("expected Struct, got {other:?}"),
        }
    }
}
