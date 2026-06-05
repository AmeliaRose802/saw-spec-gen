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

use super::types::{EnumVariant, TypeInfo};

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

        // Discriminant constrained to the *declared* set of variant
        // values. When the variants form `0, 1, …, N-1` (the default
        // for C++ `enum class` without explicit values and for Rust
        // unit enums) we emit the tight bound `var <= N-1`. When the
        // variant values have gaps (`Ok = 0, NotFound = 2, Denied =
        // 100`) we emit a disjunction `(var == v0) \/ (var == v1) \/
        // …` so SAW does not havoc bit patterns the source language
        // promises will never appear. Duplicate values (C/C++ aliases
        // like `enum { A = 1, B = 1 }`) are deduped so the disjunction
        // stays minimal.
        TypeInfo::Enum {
            variants,
            discriminant_bits,
            ..
        } if !variants.is_empty() => {
            out.push(enum_clause(var_name, variants, *discriminant_bits));
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

/// Build the Cryptol clause that clamps a symbolic enum discriminant
/// to its declared variant values.
///
/// - Empty input is a programming error (caller guards on
///   `!variants.is_empty()`).
/// - Variants are deduplicated by value so aliased members
///   (`enum { A = 1, B = 1 }`) don't blow up the disjunction.
/// - If the deduped values are exactly `0, 1, …, N-1` the clause is
///   `var <= (N-1 : [bits])` — the tight range form, same as before.
/// - Otherwise the clause is `(var == (v0 : [bits])) \/ (var == (v1 :
///   [bits])) \/ …`. Sorting by value keeps output deterministic.
fn enum_clause(var_name: &str, variants: &[EnumVariant], discriminant_bits: u32) -> String {
    let mut values: Vec<i128> = variants.iter().map(|v| v.value).collect();
    values.sort_unstable();
    values.dedup();

    if is_contiguous_from_zero(&values) {
        let max = values.last().copied().unwrap_or(0);
        return format!("{var_name} <= ({max} : [{discriminant_bits}])");
    }

    let terms: Vec<String> = values
        .iter()
        .map(|v| format!("({var_name} == ({v} : [{discriminant_bits}]))"))
        .collect();
    terms.join(" \\/ ")
}

/// True when `values` is `[0, 1, 2, …, N-1]`. Assumes the slice is
/// sorted ascending and contains no duplicates (the caller guarantees
/// both).
fn is_contiguous_from_zero(values: &[i128]) -> bool {
    values.iter().enumerate().all(|(i, v)| *v == i as i128)
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
            variants: vec![
                EnumVariant::new("Ok", 0),
                EnumVariant::new("Err", 1),
                EnumVariant::new("Pending", 2),
            ],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "status");
        assert_eq!(cs, vec!["status <= (2 : [8])"]);
    }

    #[test]
    fn enum_uses_declared_discriminant_bits() {
        let ty = TypeInfo::Enum {
            name: "Big".into(),
            variants: vec![EnumVariant::new("A", 0), EnumVariant::new("B", 1)],
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
    fn enum_gapped_values_emit_membership_disjunction() {
        // C++ `enum class Status : uint8_t { Ok = 0, NotFound = 2,
        // Denied = 100 }`. The old `<= 2` form let SAW havoc the
        // illegal value 1 (and rejected the legal value 100). The
        // membership form pins SAW to exactly the declared set.
        let ty = TypeInfo::Enum {
            name: "Status".into(),
            variants: vec![
                EnumVariant::new("Ok", 0),
                EnumVariant::new("NotFound", 2),
                EnumVariant::new("Denied", 100),
            ],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "s");
        assert_eq!(
            cs,
            vec!["(s == (0 : [8])) \\/ (s == (2 : [8])) \\/ (s == (100 : [8]))"]
        );
    }

    #[test]
    fn enum_single_high_value_emits_equality() {
        // A single non-zero variant must still emit a membership
        // clause (a `<= 100` range would allow every value below).
        let ty = TypeInfo::Enum {
            name: "Singleton".into(),
            variants: vec![EnumVariant::new("Only", 100)],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "s");
        assert_eq!(cs, vec!["(s == (100 : [8]))"]);
    }

    #[test]
    fn enum_aliased_values_deduped() {
        // `enum { A = 1, B = 1, C = 1 }` is one inhabited value, not
        // three — the disjunction collapses to a single equality.
        let ty = TypeInfo::Enum {
            name: "Aliased".into(),
            variants: vec![
                EnumVariant::new("A", 1),
                EnumVariant::new("B", 1),
                EnumVariant::new("C", 1),
            ],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "v");
        assert_eq!(cs, vec!["(v == (1 : [8]))"]);
    }

    #[test]
    fn enum_unordered_declaration_sorts_terms() {
        // Source order is irrelevant — the clause is deterministic in
        // declaration-independent (value-sorted) order so equivalent
        // inputs produce byte-identical specs.
        let ty = TypeInfo::Enum {
            name: "Reordered".into(),
            variants: vec![
                EnumVariant::new("Hundred", 100),
                EnumVariant::new("Zero", 0),
                EnumVariant::new("Two", 2),
            ],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "x");
        assert_eq!(
            cs,
            vec!["(x == (0 : [8])) \\/ (x == (2 : [8])) \\/ (x == (100 : [8]))"]
        );
    }

    #[test]
    fn enum_negative_signed_value() {
        // Signed C++ enum (`enum class Sign : int8_t { Neg = -1, Zero
        // = 0, Pos = 1 }`). Cryptol parses the literal `-1 : [8]` as
        // the two's-complement pattern — SAW handles the rest.
        let ty = TypeInfo::Enum {
            name: "Sign".into(),
            variants: vec![
                EnumVariant::new("Neg", -1),
                EnumVariant::new("Zero", 0),
                EnumVariant::new("Pos", 1),
            ],
            discriminant_bits: 8,
        };
        let cs = value_clauses(&ty, "s");
        assert_eq!(
            cs,
            vec!["(s == (-1 : [8])) \\/ (s == (0 : [8])) \\/ (s == (1 : [8]))"]
        );
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
                        variants: vec![
                            EnumVariant::new("A", 0),
                            EnumVariant::new("B", 1),
                            EnumVariant::new("C", 2),
                        ],
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
                    variants: vec![EnumVariant::new("X", 0), EnumVariant::new("Y", 1)],
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
