//! Type-driven value-constraint clauses.
//!
//! Many [`TypeInfo`] variants inhabit a strict subset of the bit
//! patterns their machine representation can hold:
//!
//! - `enum class E : u8 { A = 0, B = 1, C = 2 }` — valid values are
//!   `{0, 1, 2}`, not all 256 `u8` values.
//! - `Option<T>` and `Result<T, E>` — the discriminant tag is `{0, 1}`,
//!   not all 256 `u8` values.
//! - `Struct { e: E, ... }` — the `e` field is constrained by its own
//!   type even though the surrounding struct is otherwise unconstrained.
//!
//! [`value_clauses`] returns the Cryptol-level expressions that clamp a
//! symbolic SAW variable named `var_name` of type `ty` down to its
//! actually-inhabited value set. Callers wrap each returned clause with
//! `llvm_precond {{ … }}` (parameters) or `llvm_postcond {{ … }}`
//! (return value); the wrapping is intentionally left out so the same
//! helper can serve both sides without duplicating logic.
//!
//! Types not currently constrained (`SignedInt`, `UnsignedInt`, `Bool`,
//! `Pointer`, …) return an empty list — their SAW bit-width already
//! covers exactly the legal values. New constrained types are added by
//! extending [`push_value_clauses`].

use super::types::TypeInfo;

/// Return Cryptol expressions (without `llvm_precond` / `llvm_postcond`
/// wrapper) that clamp a symbolic variable named `var_name` of type
/// `ty` to the values that type can actually inhabit.
///
/// Returns an empty vector when the type's SAW bit-width already
/// equals its value set (integers, bools, pointers, raw byte arrays).
pub fn value_clauses(ty: &TypeInfo, var_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    push_value_clauses(ty, var_name, &mut out);
    out
}

fn push_value_clauses(ty: &TypeInfo, var_name: &str, out: &mut Vec<String>) {
    match ty {
        // Fully-occupying scalar types — every bit pattern is a valid
        // value, so no extra clause is needed.
        TypeInfo::Bool
        | TypeInfo::SignedInt(_)
        | TypeInfo::UnsignedInt(_)
        | TypeInfo::Float(_)
        | TypeInfo::ByteArray(_)
        | TypeInfo::Pointer(_)
        | TypeInfo::Opaque { .. }
        | TypeInfo::Void => {}

        // Discriminant in `[0, variants.len())`. Assumes contiguous
        // variant numbering starting at 0 — the default for C++
        // `enum class` and Rust `#[repr(uN)] enum` without explicit
        // discriminants. Variants with gaps will be over-constrained
        // until we capture explicit discriminant values too.
        TypeInfo::Enum {
            variants,
            discriminant_bits,
            ..
        } if !variants.is_empty() => {
            let max = variants.len() as u64 - 1;
            out.push(format!(
                "{var_name} <= ({max} : [{bits}])",
                bits = discriminant_bits,
            ));
        }
        TypeInfo::Enum { .. } => {} // no variants known — skip

        // Tag-only constraints. The payload (if any) is left
        // unconstrained — SAW will explore all bit patterns there.
        TypeInfo::Option(_) => {
            out.push(format!("{var_name} <= (1 : [8])"));
        }
        TypeInfo::Result(_, _) => {
            out.push(format!("{var_name} <= (1 : [8])"));
        }

        // Recurse into struct fields using Cryptol record-selector
        // syntax (`var.field`). Aggregate types that don't map cleanly
        // to a Cryptol record (sret pointer, opaque…) will yield no
        // clauses because their inner types fall through to the
        // "fully-occupying scalar" arm.
        TypeInfo::Struct { fields, .. } => {
            for (fname, fty) in fields {
                let field_path = format!("{var_name}.{fname}");
                push_value_clauses(fty, &field_path, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_types_have_no_clauses() {
        assert!(value_clauses(&TypeInfo::Bool, "b").is_empty());
        assert!(value_clauses(&TypeInfo::SignedInt(32), "i").is_empty());
        assert!(value_clauses(&TypeInfo::UnsignedInt(64), "u").is_empty());
        assert!(value_clauses(&TypeInfo::ByteArray(16), "bs").is_empty());
        assert!(
            value_clauses(&TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))), "p").is_empty()
        );
        assert!(value_clauses(&TypeInfo::Void, "v").is_empty());
    }

    #[test]
    fn enum_clamps_discriminant_to_variants_minus_one() {
        let ty = TypeInfo::Enum {
            name: "Status".into(),
            variants: vec!["Ok".into(), "Err".into(), "Pending".into()],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "status");
        assert_eq!(cs, vec!["status <= (2 : [8])"]);
    }

    #[test]
    fn enum_uses_declared_discriminant_bits() {
        let ty = TypeInfo::Enum {
            name: "Big".into(),
            variants: vec!["A".into(), "B".into()],
            discriminant_bits: 32,
        };
        let cs = value_clauses(&ty, "x");
        assert_eq!(cs, vec!["x <= (1 : [32])"]);
    }

    #[test]
    fn enum_with_no_variants_emits_nothing() {
        let ty = TypeInfo::Enum {
            name: "Empty".into(),
            variants: vec![],
            discriminant_bits: 8,
        };
        assert!(value_clauses(&ty, "x").is_empty());
    }

    #[test]
    fn option_and_result_clamp_tag() {
        let opt = TypeInfo::Option(Box::new(TypeInfo::UnsignedInt(32)));
        let res = TypeInfo::Result(
            Box::new(TypeInfo::UnsignedInt(32)),
            Box::new(TypeInfo::Bool),
        );
        assert_eq!(value_clauses(&opt, "o"), vec!["o <= (1 : [8])"]);
        assert_eq!(value_clauses(&res, "r"), vec!["r <= (1 : [8])"]);
    }

    #[test]
    fn float_types_have_no_clauses() {
        assert!(value_clauses(&TypeInfo::Float(32), "f").is_empty());
        assert!(value_clauses(&TypeInfo::Float(64), "d").is_empty());
    }

    #[test]
    fn struct_recurses_into_enum_fields() {
        // Confirms generalization: a struct that *contains* an enum
        // gets the enum's clause via Cryptol record-selector syntax,
        // even though the struct itself has no top-level clause.
        let ty = TypeInfo::Struct {
            name: "Header".into(),
            size_bytes: None,
            fields: vec![
                ("version".into(), TypeInfo::UnsignedInt(32)),
                (
                    "status".into(),
                    TypeInfo::Enum {
                        name: "Status".into(),
                        variants: vec!["A".into(), "B".into(), "C".into()],
                        discriminant_bits: 8,
                    },
                ),
            ],
        };
        let cs = value_clauses(&ty, "hdr");
        assert_eq!(cs, vec!["hdr.status <= (2 : [8])"]);
    }

    #[test]
    fn struct_recurses_through_nested_structs() {
        let inner = TypeInfo::Struct {
            name: "Inner".into(),
            size_bytes: None,
            fields: vec![(
                "tag".into(),
                TypeInfo::Enum {
                    name: "Tag".into(),
                    variants: vec!["X".into(), "Y".into()],
                    discriminant_bits: 8,
                },
            )],
        };
        let outer = TypeInfo::Struct {
            name: "Outer".into(),
            size_bytes: None,
            fields: vec![("inner".into(), inner)],
        };
        let cs = value_clauses(&outer, "o");
        assert_eq!(cs, vec!["o.inner.tag <= (1 : [8])"]);
    }
}
