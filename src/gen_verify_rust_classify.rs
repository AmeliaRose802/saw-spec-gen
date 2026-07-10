//! Return-type classification helpers for the `gen-verify-rust` subcommand.

use crate::constraints::TypeInfo;
use crate::gen_verify_rust_emit::RustReturnKind;

/// Classify a non-sret return type into the appropriate [`RustReturnKind`].
/// Returns `None` if the type cannot be handled (e.g. void or pointer-only).
pub fn classify_return(ty: &TypeInfo) -> Option<RustReturnKind> {
    match ty {
        TypeInfo::Bool => Some(RustReturnKind::Scalar { bits: 1 }),
        TypeInfo::SignedInt(n) | TypeInfo::UnsignedInt(n) => {
            Some(RustReturnKind::Scalar { bits: *n })
        }
        TypeInfo::Struct {
            name,
            size_bytes,
            fields,
        } => {
            // Try aggregate (all fields are iN)
            let field_bits: Option<Vec<u32>> =
                fields.iter().map(|(_, ft)| type_int_bits(ft)).collect();
            if let Some(fb) = field_bits {
                return Some(RustReturnKind::Aggregate { field_bits: fb });
            }
            // Fallback: sret with known size
            if let Some(sz) = size_bytes {
                if *sz > 0 {
                    return Some(RustReturnKind::Sret {
                        llvm_type: format!("%{name}"),
                        size_bytes: *sz,
                    });
                }
            }
            None
        }
        // Inline aggregate type like "{ i1, i1 }" (no named struct in IR).
        // LLVM represents anonymous inline struct types with literal brace
        // syntax; named types use "%TypeName" which maps to TypeInfo::Struct.
        TypeInfo::Opaque { name, .. } if name.starts_with('{') => {
            classify_inline_struct_return(name)
        }
        _ => None,
    }
}

/// Classify the promoted return type of an sret function.  Handles
/// byte-array sret types (`[N x i8]`) from newer LLVM versions and
/// named struct types (`%Foo`) from older ones.
pub fn classify_sret_return(ty: &TypeInfo) -> Option<RustReturnKind> {
    match ty {
        TypeInfo::ByteArray(n) => Some(RustReturnKind::Sret {
            llvm_type: format!("[{n} x i8]"),
            size_bytes: *n,
        }),
        TypeInfo::Struct {
            name, size_bytes, ..
        } => Some(RustReturnKind::Sret {
            llvm_type: format!("%{name}"),
            size_bytes: size_bytes.unwrap_or(0),
        }),
        TypeInfo::Opaque { name, size_bytes } => Some(RustReturnKind::Sret {
            llvm_type: name.clone(),
            size_bytes: *size_bytes,
        }),
        _ => None,
    }
}

/// Parse an inline LLVM struct type like `{ i1, i1 }` or `{ i8, i32 }`.
pub fn classify_inline_struct_return(type_str: &str) -> Option<RustReturnKind> {
    let inner = type_str.trim().strip_prefix('{')?.strip_suffix('}')?.trim();
    if inner.is_empty() {
        return None;
    }
    let field_bits: Option<Vec<u32>> = inner
        .split(',')
        .map(|s| match s.trim() {
            "i1" => Some(1u32),
            "i8" => Some(8),
            "i16" => Some(16),
            "i32" => Some(32),
            "i64" => Some(64),
            _ => None,
        })
        .collect();
    Some(RustReturnKind::Aggregate {
        field_bits: field_bits?,
    })
}

pub fn format_return_llvm_type(kind: &RustReturnKind) -> String {
    match kind {
        RustReturnKind::Scalar { bits } => format!("i{bits}"),
        RustReturnKind::Sret { llvm_type, .. } => llvm_type.clone(),
        RustReturnKind::Aggregate { field_bits } => {
            let fields = field_bits
                .iter()
                .map(|b| format!("i{b}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
    }
}

/// Width in bits of an `iN`-shaped [`TypeInfo`], or `None` for any
/// other type.
pub fn type_int_bits(t: &TypeInfo) -> Option<u32> {
    match t {
        TypeInfo::Bool => Some(1),
        TypeInfo::SignedInt(n) | TypeInfo::UnsignedInt(n) => Some(*n),
        _ => None,
    }
}
