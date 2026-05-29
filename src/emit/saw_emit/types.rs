//! Type-string mapping helpers used across spec emitters.
//!
//! Bridges three closely related but distinct type vocabularies:
//!
//! * **SAW LLVM**: `llvm_int 32`, `llvm_array N T`, `llvm_alias "Foo"`.
//! * **SAW MIR**: `mir_u32`, `mir_array N T`, `mir_find_adt m "Foo"`.
//! * **LLVM IR text**: `i32`, `[N x i8]`, `ptr`, `void`.

use crate::constraints::{FunctionInfo, TypeInfo};

/// Map an LLVM SAW type string to the corresponding MIR SAW form.
pub fn mir_saw_type(llvm_type: &str) -> String {
    if let Some(bits) = llvm_type.strip_prefix("llvm_int ") {
        format!("mir_u{bits}")
    } else if llvm_type.starts_with("llvm_array ") {
        llvm_type
            .replace("llvm_array", "mir_array")
            .replace("llvm_int", "mir_u")
    } else if let Some(rest) = llvm_type.strip_prefix("llvm_alias ") {
        let name = rest.trim_start_matches('"').trim_end_matches('"');
        format!("mir_find_adt m \"{name}\"")
    } else {
        llvm_type.to_string()
    }
}

/// Map a SAW LLVM type string to a Cryptol type for use in TODO comments.
/// Currently only handles the common `llvm_int N` case; everything else
/// passes through verbatim.
pub fn cryptol_type_for_saw(saw_type: &str) -> String {
    if let Some(bits) = saw_type.strip_prefix("llvm_int ") {
        format!("[{bits}]")
    } else {
        saw_type.to_string()
    }
}

/// Convert a [`TypeInfo`] to its LLVM IR-text representation.
///
/// `Opaque` values with `size_bytes > 0 && <= 8` are treated as scalar
/// integers (most often this means an enum that was parsed as opaque);
/// larger opaques fall through to a byte array; unknown small types
/// default to `i32`.
pub fn type_to_llvm_ir(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Void => "void".into(),
        TypeInfo::Bool => "i1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("i{bits}"),
        TypeInfo::Pointer(_) => "ptr".into(),
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => format!("[{n} x i8]"),
        TypeInfo::ByteArray(n) => format!("[{n} x i8]"),
        TypeInfo::Enum {
            discriminant_bits, ..
        } => format!("i{discriminant_bits}"),
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 0 && *size_bytes <= 8 => {
            format!("i{}", size_bytes * 8)
        }
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8 => {
            format!("[{size_bytes} x i8]")
        }
        _ => "i32".into(),
    }
}

/// Returns `Some(inner_ir_type)` when a return type is lowered to an
/// `sret` pointer parameter under the MSVC x64 ABI.
///
/// MSVC returns aggregates larger than 8 bytes via an implicit hidden
/// pointer parameter. We conservatively treat any `Struct` (sized or
/// unsized) as sret because aggregate returns in C++ classes always
/// use the hidden-pointer convention. Templated/qualified `Opaque`
/// returns are likewise treated as aggregates (see
/// [`looks_like_aggregate_name`]).
pub fn sret_inner_ir_type(ty: &TypeInfo) -> Option<String> {
    match ty {
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => Some(format!("[{n} x i8]")),
        TypeInfo::Struct {
            size_bytes: None, ..
        } => Some("[16 x i8]".to_string()),
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8 => {
            Some(format!("[{size_bytes} x i8]"))
        }
        TypeInfo::Opaque {
            size_bytes: 0,
            name,
        } if looks_like_aggregate_name(name) => Some("[16 x i8]".to_string()),
        _ => None,
    }
}

/// Heuristic: does this opaque type name almost certainly denote a C++
/// aggregate (struct/class/template instantiation)?
///
/// Used as a fallback when `size_bytes` isn't known. Templated names
/// (`std::tuple<…>`) and fully-qualified C++ names are always class
/// types under MSVC. Plain unqualified opaques like `Self` / `Unknown`
/// (used for unresolved abstract `this` placeholders) are NOT aggregates.
pub fn looks_like_aggregate_name(name: &str) -> bool {
    name.contains('<') || name.contains("::")
}

/// Per-parameter LLVM IR pieces for a method's stub signature.
///
/// Returned as a `Vec<String>` so callers can splice the implicit
/// `this` and an `sret` pointer at the right positions before joining.
pub fn method_param_ir_pieces(func: &FunctionInfo) -> Vec<String> {
    func.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match &p.ty {
                TypeInfo::Pointer(_) => "ptr".to_string(),
                other => type_to_llvm_ir(other),
            };
            format!("{ty} %arg{i}")
        })
        .collect()
}

/// Default return instruction for an LLVM IR type. Used to give the
/// stub body something well-typed; the override replaces the body so
/// this value is never observed at runtime.
pub fn ir_default_return(ir_type: &str) -> String {
    match ir_type {
        "void" => "ret void".into(),
        "i1" => "ret i1 false".into(),
        t => format!("ret {t} 0"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mir_saw_type_maps_scalars() {
        assert_eq!(mir_saw_type("llvm_int 32"), "mir_u32");
        assert_eq!(mir_saw_type("llvm_int 8"), "mir_u8");
        assert_eq!(mir_saw_type("llvm_int 64"), "mir_u64");
    }

    #[test]
    fn mir_saw_type_maps_alias() {
        assert_eq!(
            mir_saw_type("llvm_alias \"MyStruct\""),
            "mir_find_adt m \"MyStruct\""
        );
    }

    #[test]
    fn mir_saw_type_passes_through_unknown() {
        assert_eq!(mir_saw_type("// void"), "// void");
        assert_eq!(mir_saw_type("custom"), "custom");
    }

    #[test]
    fn cryptol_type_for_llvm_int() {
        assert_eq!(cryptol_type_for_saw("llvm_int 32"), "[32]");
        assert_eq!(cryptol_type_for_saw("llvm_int 8"), "[8]");
    }

    #[test]
    fn type_to_llvm_ir_handles_primitives() {
        assert_eq!(type_to_llvm_ir(&TypeInfo::Bool), "i1");
        assert_eq!(type_to_llvm_ir(&TypeInfo::SignedInt(32)), "i32");
        assert_eq!(type_to_llvm_ir(&TypeInfo::UnsignedInt(64)), "i64");
        assert_eq!(type_to_llvm_ir(&TypeInfo::Void), "void");
        assert_eq!(
            type_to_llvm_ir(&TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8)))),
            "ptr"
        );
    }

    #[test]
    fn type_to_llvm_ir_aggregates_become_byte_arrays() {
        assert_eq!(
            type_to_llvm_ir(&TypeInfo::Struct {
                name: "Foo".into(),
                size_bytes: Some(24),
                fields: vec![]
            }),
            "[24 x i8]"
        );
        assert_eq!(type_to_llvm_ir(&TypeInfo::ByteArray(16)), "[16 x i8]");
    }

    #[test]
    fn looks_like_aggregate_recognizes_templates_and_namespaces() {
        assert!(looks_like_aggregate_name("std::tuple<int>"));
        assert!(looks_like_aggregate_name("foo::Bar"));
        assert!(looks_like_aggregate_name("Container<T>"));
        assert!(!looks_like_aggregate_name("Self"));
        assert!(!looks_like_aggregate_name("Unknown"));
    }

    #[test]
    fn sret_required_for_large_structs() {
        assert_eq!(
            sret_inner_ir_type(&TypeInfo::Struct {
                name: "T".into(),
                size_bytes: Some(48),
                fields: vec![],
            }),
            Some("[48 x i8]".to_string())
        );
    }

    #[test]
    fn sret_required_for_unsized_aggregates() {
        assert_eq!(
            sret_inner_ir_type(&TypeInfo::Opaque {
                name: "std::tuple<A,B>".into(),
                size_bytes: 0,
            }),
            Some("[16 x i8]".to_string())
        );
    }

    #[test]
    fn sret_not_required_for_scalar_or_pointer() {
        assert_eq!(sret_inner_ir_type(&TypeInfo::SignedInt(32)), None);
        assert_eq!(
            sret_inner_ir_type(&TypeInfo::Pointer(Box::new(TypeInfo::Bool))),
            None
        );
        assert_eq!(
            sret_inner_ir_type(&TypeInfo::Opaque {
                name: "Self".into(),
                size_bytes: 0,
            }),
            None
        );
    }

    #[test]
    fn ir_default_return_per_type() {
        assert_eq!(ir_default_return("void"), "ret void");
        assert_eq!(ir_default_return("i1"), "ret i1 false");
        assert_eq!(ir_default_return("i32"), "ret i32 0");
        assert_eq!(ir_default_return("ptr"), "ret ptr 0");
    }
}
