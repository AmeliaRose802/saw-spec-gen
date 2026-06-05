//! Rust type-string parser used by mir-json processing.
//!
//! mir-json emits types as opaque strings (`"ty::Int::I32"`,
//! `"Option<u32>"`, `"ty::Ref<'_, u8, Shared>"`, etc.). This module
//! turns those into [`TypeInfo`] values + mutability + nullability
//! triples.

use crate::constraints::{Mutability, Nullability, TypeInfo};
use std::collections::HashMap;

/// Parse a Rust type string. Returns the type plus the implied
/// mutability and nullability from the type-level information
/// (references → `Readonly` unless `is_mut`, raw pointers → `Nullable`,
/// everything else → `Readonly`/`NonNull`).
pub fn parse_rust_type_string(
    ty_name: &str,
    is_mut: bool,
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    if ty_name.starts_with("ty::Ref") {
        return parse_reference(ty_name, is_mut, adt_map);
    }
    if ty_name.contains("RawPtr") {
        return parse_raw_pointer(ty_name, is_mut, adt_map);
    }
    if is_option_name(ty_name) {
        return parse_option(ty_name, is_mut, adt_map);
    }
    if is_result_name(ty_name) {
        return parse_result(ty_name, is_mut, adt_map);
    }
    let ty = match_primitive_or_adt(ty_name, adt_map);
    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };
    (ty, mutability, Nullability::NonNull)
}

fn parse_reference(
    ty_name: &str,
    is_mut: bool,
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };
    let inner_ty = extract_ref_inner_type(ty_name, adt_map);
    (
        TypeInfo::Pointer(Box::new(inner_ty)),
        mutability,
        Nullability::NonNull,
    )
}

fn parse_raw_pointer(
    ty_name: &str,
    is_mut: bool,
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };
    let inner_ty = extract_ref_inner_type(ty_name, adt_map);
    (
        TypeInfo::Pointer(Box::new(inner_ty)),
        mutability,
        Nullability::Nullable,
    )
}

fn is_option_name(ty_name: &str) -> bool {
    ty_name.starts_with("Option<") || ty_name.contains("::Option<")
}

fn is_result_name(ty_name: &str) -> bool {
    ty_name.starts_with("Result<") || ty_name.contains("::Result<")
}

fn parse_option(
    ty_name: &str,
    is_mut: bool,
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    let inner_name = ty_name
        .split('<')
        .nth(1)
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or("()");
    let (inner_ty, _, _) = parse_rust_type_string(inner_name, false, adt_map);
    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };
    (
        TypeInfo::Option(Box::new(inner_ty)),
        mutability,
        Nullability::NonNull,
    )
}

fn parse_result(
    ty_name: &str,
    is_mut: bool,
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    let inner = ty_name
        .split('<')
        .nth(1)
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or("(), ()");
    let parts: Vec<&str> = inner.splitn(2, ", ").collect();
    let ok_name = parts.first().copied().unwrap_or("()");
    let err_name = parts.get(1).copied().unwrap_or("()");
    let (ok_ty, _, _) = parse_rust_type_string(ok_name, false, adt_map);
    let (err_ty, _, _) = parse_rust_type_string(err_name, false, adt_map);
    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };
    (
        TypeInfo::Result(Box::new(ok_ty), Box::new(err_ty)),
        mutability,
        Nullability::NonNull,
    )
}

fn match_primitive_or_adt(ty_name: &str, adt_map: &HashMap<String, TypeInfo>) -> TypeInfo {
    if let Some(ty) = match_primitive(ty_name) {
        return ty;
    }
    if let Some(adt_ty) = adt_map.get(ty_name) {
        return rename_adt(adt_ty, ty_name);
    }
    TypeInfo::Opaque {
        name: ty_name.into(),
        size_bytes: 0,
    }
}

/// Set the `name` field on an ADT type to match the lookup key. mir-json
/// emits the same struct/enum twice — once on the ADT def (with the
/// name) and again at the use site (where the name lives only in the
/// type string). Pre-populating the name here keeps downstream emitters
/// from having to remember which side wrote it.
fn rename_adt(adt_ty: &TypeInfo, name: &str) -> TypeInfo {
    match adt_ty {
        TypeInfo::Enum {
            variants,
            discriminant_bits,
            ..
        } => TypeInfo::Enum {
            name: name.to_string(),
            variants: variants.clone(),
            discriminant_bits: *discriminant_bits,
        },
        TypeInfo::Struct {
            size_bytes, fields, ..
        } => TypeInfo::Struct {
            name: name.to_string(),
            size_bytes: *size_bytes,
            fields: fields.clone(),
        },
        other => other.clone(),
    }
}

fn match_primitive(ty_name: &str) -> Option<TypeInfo> {
    Some(match ty_name {
        "ty::Bool" | "bool" => TypeInfo::Bool,
        "ty::Uint::U8" | "u8" => TypeInfo::UnsignedInt(8),
        "ty::Uint::U16" | "u16" => TypeInfo::UnsignedInt(16),
        "ty::Uint::U32" | "u32" => TypeInfo::UnsignedInt(32),
        "ty::Uint::U64" | "u64" => TypeInfo::UnsignedInt(64),
        "ty::Uint::Usize" | "usize" => TypeInfo::UnsignedInt(64),
        "ty::Int::I8" | "i8" => TypeInfo::SignedInt(8),
        "ty::Int::I16" | "i16" => TypeInfo::SignedInt(16),
        "ty::Int::I32" | "i32" => TypeInfo::SignedInt(32),
        "ty::Int::I64" | "i64" => TypeInfo::SignedInt(64),
        "ty::Int::Isize" | "isize" => TypeInfo::SignedInt(64),
        "ty::Float::F32" | "f32" => TypeInfo::Float(32),
        "ty::Float::F64" | "f64" => TypeInfo::Float(64),
        "()" | "ty::Tuple::unit" => TypeInfo::Void,
        _ => return None,
    })
}

/// Extract the pointee type from a reference/pointer type string.
/// Example: `"ty::Ref<'_, u8, Shared>"` → `u8`.
pub fn extract_ref_inner_type(ty_name: &str, adt_map: &HashMap<String, TypeInfo>) -> TypeInfo {
    if let Some(start) = ty_name.find('<') {
        let inner = &ty_name[start + 1..];
        if let Some(end) = inner.rfind('>') {
            let parts: Vec<&str> = inner[..end].split(", ").collect();
            // mir-json shape: <'lifetime, Type, Mutability>
            if parts.len() >= 2 {
                let inner_name = parts[1].trim();
                let (ty, _, _) = parse_rust_type_string(inner_name, false, adt_map);
                return ty;
            }
        }
    }
    TypeInfo::Opaque {
        name: ty_name.into(),
        size_bytes: 0,
    }
}

/// Parse a `return_ty` JSON value (a string in current mir-json) into a
/// [`TypeInfo`]. Discards the mutability/nullability triple because
/// return types don't carry those.
pub fn parse_mir_return_type(
    ty_val: &serde_json::Value,
    adt_map: &HashMap<String, TypeInfo>,
) -> Option<TypeInfo> {
    let ty_name = ty_val.as_str()?;
    let (ty, _, _) = parse_rust_type_string(ty_name, false, adt_map);
    Some(ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::EnumVariant;

    #[test]
    fn primitives_map_to_typed_int_widths() {
        let m = HashMap::new();
        let (t, _, _) = parse_rust_type_string("u32", false, &m);
        assert_eq!(t, TypeInfo::UnsignedInt(32));
        let (t, _, _) = parse_rust_type_string("ty::Int::I64", false, &m);
        assert_eq!(t, TypeInfo::SignedInt(64));
        let (t, _, _) = parse_rust_type_string("bool", false, &m);
        assert_eq!(t, TypeInfo::Bool);
        let (t, _, _) = parse_rust_type_string("()", false, &m);
        assert_eq!(t, TypeInfo::Void);
    }

    #[test]
    fn shared_reference_is_readonly_nonnull() {
        let m = HashMap::new();
        let (t, mu, nu) = parse_rust_type_string("ty::Ref<'_, u32, Shared>", false, &m);
        assert!(matches!(t, TypeInfo::Pointer(_)));
        assert_eq!(mu, Mutability::Readonly);
        assert_eq!(nu, Nullability::NonNull);
    }

    #[test]
    fn mutable_reference_is_mutable_nonnull() {
        let m = HashMap::new();
        let (_, mu, nu) = parse_rust_type_string("ty::Ref<'_, u32, Mut>", true, &m);
        assert_eq!(mu, Mutability::Mutable);
        assert_eq!(nu, Nullability::NonNull);
    }

    #[test]
    fn raw_pointer_is_nullable() {
        let m = HashMap::new();
        let (t, _, nu) = parse_rust_type_string("ty::RawPtr<u8>", false, &m);
        assert!(matches!(t, TypeInfo::Pointer(_)));
        assert_eq!(nu, Nullability::Nullable);
    }

    #[test]
    fn option_inner_resolves_through_primitives() {
        let m = HashMap::new();
        let (t, _, _) = parse_rust_type_string("Option<u32>", false, &m);
        match t {
            TypeInfo::Option(inner) => assert_eq!(*inner, TypeInfo::UnsignedInt(32)),
            other => panic!("expected Option, got {other:?}"),
        }
    }

    #[test]
    fn result_inner_resolves_both_variants() {
        let m = HashMap::new();
        let (t, _, _) = parse_rust_type_string("Result<u32, i8>", false, &m);
        match t {
            TypeInfo::Result(ok, err) => {
                assert_eq!(*ok, TypeInfo::UnsignedInt(32));
                assert_eq!(*err, TypeInfo::SignedInt(8));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn adt_map_lookup_assigns_canonical_name() {
        let mut m = HashMap::new();
        m.insert(
            "MyEnum".to_string(),
            TypeInfo::Enum {
                name: String::new(),
                variants: vec![EnumVariant::new("A", 0), EnumVariant::new("B", 1)],
                discriminant_bits: 32,
            },
        );
        let (t, _, _) = parse_rust_type_string("MyEnum", false, &m);
        match t {
            TypeInfo::Enum { name, .. } => assert_eq!(name, "MyEnum"),
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn unknown_type_becomes_opaque() {
        let m = HashMap::new();
        let (t, _, _) = parse_rust_type_string("FooBar", false, &m);
        assert!(matches!(t, TypeInfo::Opaque { ref name, .. } if name == "FooBar"));
    }

    #[test]
    fn extract_ref_inner_handles_lifetime_form() {
        let m = HashMap::new();
        let t = extract_ref_inner_type("ty::Ref<'_, u8, Shared>", &m);
        assert_eq!(t, TypeInfo::UnsignedInt(8));
    }

    #[test]
    fn float_types_map_to_float_variant() {
        let m = HashMap::new();
        let (t, _, _) = parse_rust_type_string("f32", false, &m);
        assert_eq!(t, TypeInfo::Float(32));
        let (t, _, _) = parse_rust_type_string("f64", false, &m);
        assert_eq!(t, TypeInfo::Float(64));
        let (t, _, _) = parse_rust_type_string("ty::Float::F32", false, &m);
        assert_eq!(t, TypeInfo::Float(32));
        let (t, _, _) = parse_rust_type_string("ty::Float::F64", false, &m);
        assert_eq!(t, TypeInfo::Float(64));
    }
}
