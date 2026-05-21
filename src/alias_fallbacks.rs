//! Collects fallback sizes used by `spec_rewrite` to replace
//! unresolvable `llvm_alias "X"` references in generated SAW specs.
//!
//! See `spec_rewrite` for the rewriter that consumes the
//! [`AliasFallbacks`] produced by [`collect_type_sizes`].

use crate::constraints::{Annotation, FunctionInfo, TypeInfo};
use std::collections::HashMap;

/// Replacement strategies for an unresolvable `llvm_alias "X"`.
///
/// Most C++ types lower to a sized byte buffer (`bytes`), which is sound
/// for override / havoc specs because SAW treats the allocation as an
/// opaque blob.  Enums that lower to a single LLVM integer in the ABI
/// (the common case on x64 — `i32`) need `enum_bits` so SAW can match
/// the typed store the bitcode performs against the allocation; a byte
/// array would trip "types not memory-compatible" when the i32 store
/// lands.
#[derive(Debug, Default, Clone)]
pub struct AliasFallbacks {
    /// `name → bytes`, emitted as `llvm_array N (llvm_int 8)`.
    pub bytes: HashMap<String, usize>,
    /// `name → bit width`, emitted as `llvm_int N` (enums in the ABI).
    pub enum_bits: HashMap<String, u32>,
}

impl AliasFallbacks {
    /// Insert a byte-size fallback, keeping the largest value seen for
    /// `name`.  Larger is safer because override allocations are treated
    /// as opaque blobs — under-allocating risks SAW reading past the end
    /// during havoc.
    pub(crate) fn insert_bytes(&mut self, name: &str, size: usize) {
        let entry = self.bytes.entry(name.to_string()).or_insert(size);
        if size > *entry {
            *entry = size;
        }
    }

    /// Insert an enum bit-width fallback, keeping the largest width seen.
    fn insert_enum(&mut self, name: &str, bits: u32) {
        let entry = self.enum_bits.entry(name.to_string()).or_insert(bits);
        if bits > *entry {
            *entry = bits;
        }
    }
}

/// Walk a slice of `FunctionInfo` values and record every `name → fallback`
/// hint we can use when an `llvm_alias "X"` reference can't be resolved
/// against the LLVM IR struct table.
///
/// Sources contributing fallbacks:
///   - `TypeInfo::Struct { size_bytes: Some(n), .. }` — clang AST struct
///     definitions where every field has a known C++ size.
///   - `TypeInfo::Opaque { size_bytes: n>0, .. }` — well-known STL /
///     platform types looked up via `clang_ast::lookup_known_type_size`
///     (`std::string` → 32, `std::vector<…>` → 24, `GUID` → 16, …).
///   - `TypeInfo::Enum { discriminant_bits, .. }` — recorded as an enum
///     fallback so the rewriter emits `llvm_int <bits>` (matching the
///     ABI lowering) rather than a byte array.
///   - `Annotation::Dereferenceable(N)` on a parameter whose pointee is
///     a named struct / opaque / enum — covers the common case where
///     the AST only has a forward declaration of the type
///     (`HttpRequest`, `std::tuple<…>`) and clang therefore can't
///     compute the layout.  The LLVM IR's `dereferenceable(N)`
///     attribute is the only reliable source of the type's allocation
///     size in that scenario.
///
/// The map keys are the *short* C++ names that `type_to_saw` emits as
/// `llvm_alias "X"`; the rewriter looks them up verbatim.
pub fn collect_type_sizes(funcs: &[FunctionInfo]) -> AliasFallbacks {
    let mut out = AliasFallbacks::default();
    for f in funcs {
        for p in &f.params {
            walk_type_for_sizes(&p.ty, &mut out);
            // `dereferenceable(N)` gives us the byte size of the pointee
            // — invaluable when the AST only had a forward declaration
            // and the field layout is unavailable.
            if let Some(n) = deref_annotation(&p.annotations) {
                if let Some(name) = pointee_name(&p.ty) {
                    out.insert_bytes(name, n);
                }
            }
        }
        walk_type_for_sizes(&f.return_type, &mut out);
        for g in &f.referenced_globals {
            walk_type_for_sizes(&g.ty, &mut out);
        }
    }
    out
}

/// If `anns` contains a `Dereferenceable(N)` annotation, return `N`.
pub(crate) fn deref_annotation(anns: &[Annotation]) -> Option<usize> {
    anns.iter().find_map(|a| match a {
        Annotation::Dereferenceable(n) => Some(*n),
        _ => None,
    })
}

/// Extract the short C++ name of a (possibly pointer-wrapped) named type.
/// Returns `None` for scalars / arrays / `Option` / `Result` / `Void`.
pub(crate) fn pointee_name(ty: &TypeInfo) -> Option<&str> {
    let target = match ty {
        TypeInfo::Pointer(inner) => inner.as_ref(),
        other => other,
    };
    match target {
        TypeInfo::Struct { name, .. }
        | TypeInfo::Opaque { name, .. }
        | TypeInfo::Enum { name, .. } => Some(name.as_str()),
        _ => None,
    }
}

/// Recursive helper for `collect_type_sizes`.
fn walk_type_for_sizes(ty: &TypeInfo, out: &mut AliasFallbacks) {
    match ty {
        TypeInfo::Struct {
            name,
            size_bytes: Some(n),
            fields,
        } => {
            out.insert_bytes(name, *n);
            for (_, fty) in fields {
                walk_type_for_sizes(fty, out);
            }
        }
        TypeInfo::Struct { name, fields, .. } => {
            // Try lookup_known_type_size for templated STL names that
            // may not have field-derived size (e.g. `std::vector<…>`).
            // `std::tuple<…>` returns None here and must be sized via
            // the parameter's `dereferenceable(N)` annotation instead.
            if let Some(n) = crate::clang_ast::lookup_known_type_size(name) {
                out.insert_bytes(name, n);
            }
            for (_, fty) in fields {
                walk_type_for_sizes(fty, out);
            }
        }
        TypeInfo::Opaque {
            name,
            size_bytes: n,
        } if *n > 0 => {
            out.insert_bytes(name, *n);
        }
        TypeInfo::Opaque { name, .. } => {
            if let Some(n) = crate::clang_ast::lookup_known_type_size(name) {
                out.insert_bytes(name, n);
            }
        }
        TypeInfo::Enum {
            name,
            discriminant_bits,
            ..
        } => {
            // Record the integer width so the rewriter can emit
            // `llvm_int <bits>` (matching the ABI lowering of the enum
            // to a single LLVM integer).
            out.insert_enum(name, (*discriminant_bits).max(1));
        }
        TypeInfo::Pointer(inner) => walk_type_for_sizes(inner, out),
        TypeInfo::Option(inner) => walk_type_for_sizes(inner, out),
        TypeInfo::Result(ok, err) => {
            walk_type_for_sizes(ok, out);
            walk_type_for_sizes(err, out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{Mutability, Nullability, ParamInfo};

    fn empty_func() -> FunctionInfo {
        FunctionInfo {
            name: "f".into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    fn param(name: &str, ty: TypeInfo) -> ParamInfo {
        ParamInfo {
            name: name.into(),
            ty,
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }
    }

    #[test]
    fn collect_sizes_picks_up_struct_with_size() {
        let mut f = empty_func();
        f.params.push(param(
            "p",
            TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: "MyStruct".into(),
                size_bytes: Some(24),
                fields: vec![],
            })),
        ));
        let fb = collect_type_sizes(&[f]);
        assert_eq!(fb.bytes.get("MyStruct").copied(), Some(24));
    }

    #[test]
    fn collect_sizes_picks_up_enum_discriminant_bits() {
        let mut f = empty_func();
        f.params.push(param(
            "s",
            TypeInfo::Enum {
                name: "SessionState".into(),
                variants: vec!["A".into(), "B".into()],
                discriminant_bits: 32,
            },
        ));
        let fb = collect_type_sizes(&[f]);
        assert_eq!(fb.enum_bits.get("SessionState").copied(), Some(32));
        // Enums must NOT be recorded in the bytes map — the rewriter
        // emits `llvm_int N` for them, not `llvm_array N (llvm_int 8)`.
        assert!(fb.bytes.get("SessionState").is_none());
    }

    #[test]
    fn collect_sizes_picks_up_opaque_with_known_stl_name() {
        let mut f = empty_func();
        f.params.push(param(
            "s",
            TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "std::string".into(),
                size_bytes: 0,
            })),
        ));
        let fb = collect_type_sizes(&[f]);
        assert_eq!(fb.bytes.get("std::string").copied(), Some(32));
    }

    #[test]
    fn collect_sizes_uses_dereferenceable_for_forward_declared_struct() {
        // `HttpRequest` is a forward-declared class: clang has no field
        // layout, so `TypeInfo::Opaque { size_bytes: 0 }`.  The LLVM IR
        // attribute `dereferenceable(112)` on the parameter is the only
        // reliable source of its allocation size.
        let mut f = empty_func();
        let mut p = param(
            "req",
            TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "HttpRequest".into(),
                size_bytes: 0,
            })),
        );
        p.annotations.push(Annotation::Dereferenceable(112));
        f.params.push(p);
        let fb = collect_type_sizes(&[f]);
        assert_eq!(fb.bytes.get("HttpRequest").copied(), Some(112));
    }

    #[test]
    fn collect_sizes_uses_dereferenceable_for_std_tuple() {
        // `std::tuple<…>` has no entry in `lookup_known_type_size` — the
        // ABI size depends on the template parameters.  The sret hidden
        // parameter carries `dereferenceable(N)`, which is the only
        // reliable source of the allocation size in this case.
        let mut f = empty_func();
        let tuple_name =
            "std::tuple<KeyStoreOperationResult, LatchableKey>".to_string();
        let mut p = param(
            "result",
            TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: tuple_name.clone(),
                size_bytes: None,
                fields: vec![],
            })),
        );
        p.annotations.push(Annotation::Dereferenceable(48));
        f.params.push(p);
        let fb = collect_type_sizes(&[f]);
        assert_eq!(fb.bytes.get(&tuple_name).copied(), Some(48));
    }

    #[test]
    fn collect_sizes_dereferenceable_takes_max_when_repeated() {
        // Two callsites give different `dereferenceable` values for the
        // same forward-declared type — prefer the larger size to avoid
        // under-allocating in any single override.
        let mut f1 = empty_func();
        let mut p1 = param(
            "x",
            TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "Foo".into(),
                size_bytes: 0,
            })),
        );
        p1.annotations.push(Annotation::Dereferenceable(8));
        f1.params.push(p1);
        let mut f2 = empty_func();
        let mut p2 = param(
            "y",
            TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "Foo".into(),
                size_bytes: 0,
            })),
        );
        p2.annotations.push(Annotation::Dereferenceable(64));
        f2.params.push(p2);
        let fb = collect_type_sizes(&[f1, f2]);
        assert_eq!(fb.bytes.get("Foo").copied(), Some(64));
    }
}
