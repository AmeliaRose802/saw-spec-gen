//! Parse mir-json output and extract function signatures with Rust type info.
//!
//! mir-json serializes Rust MIR to JSON, preserving full type information
//! including enum variants, reference mutability, lifetimes, etc.

use crate::constraints::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Parse a mir-json output file.
pub fn parse_mir(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mir: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
    Ok(mir)
}

/// Extract function info from MIR JSON.
pub fn extract_functions(mir: &Value, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let mut functions = Vec::new();

    // mir-json linked format has a top-level "fns" array
    if let Some(fns) = mir.get("fns").and_then(|v| v.as_array()) {
        for func_val in fns {
            if let Some(func) = parse_mir_function(func_val) {
                let matches = filter
                    .map(|f| func.name.contains(f))
                    .unwrap_or(true);
                if matches {
                    functions.push(func);
                }
            }
        }
    }

    Ok(functions)
}

fn parse_mir_function(func: &Value) -> Option<FunctionInfo> {
    let name = func.get("name")?.as_str()?.to_string();
    let abi = func.get("abi").and_then(|v| v.get("kind")).and_then(|v| v.as_str());

    let mut params = Vec::new();

    if let Some(args) = func.get("args").and_then(|v| v.as_array()) {
        for arg in args {
            if let Some(param) = parse_mir_param(arg) {
                params.push(param);
            }
        }
    }

    let return_type = func
        .get("return_ty")
        .and_then(|v| parse_mir_type(v))
        .unwrap_or(TypeInfo::Void);

    Some(FunctionInfo {
        name,
        mangled_name: None,
        params,
        return_type,
        can_throw: false, // Rust doesn't have exceptions (panics are different)
        annotations: Vec::new(),
    })
}

fn parse_mir_param(arg: &Value) -> Option<ParamInfo> {
    let name = arg.get("name")?.as_str()?.to_string();
    let ty_name = arg.get("ty")?.as_str()?;
    let is_mut = arg
        .get("mut")
        .and_then(|v| v.get("kind"))
        .and_then(|v| v.as_str())
        == Some("Mut");

    let (ty, mutability, nullable) = parse_rust_type_string(ty_name, is_mut);

    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations: Vec::new(),
    })
}

fn parse_rust_type_string(
    ty_name: &str,
    is_mut: bool,
) -> (TypeInfo, Mutability, Nullability) {
    // Rust references: &T is readonly, &mut T is mutable, both are non-null
    if ty_name.starts_with("ty::Ref") {
        let mutability = if is_mut {
            Mutability::Mutable
        } else {
            Mutability::Readonly
        };
        return (
            TypeInfo::Opaque {
                name: ty_name.into(),
                size_bytes: 0,
            },
            mutability,
            Nullability::NonNull,
        );
    }

    // Raw pointers: *const T and *mut T — nullable
    if ty_name.contains("RawPtr") {
        let mutability = if is_mut {
            Mutability::Mutable
        } else {
            Mutability::Readonly
        };
        return (
            TypeInfo::Opaque {
                name: ty_name.into(),
                size_bytes: 0,
            },
            mutability,
            Nullability::Nullable,
        );
    }

    // Common Rust types
    let ty = match ty_name {
        "ty::Bool" => TypeInfo::Bool,
        "ty::Uint::U8" => TypeInfo::UnsignedInt(8),
        "ty::Uint::U16" => TypeInfo::UnsignedInt(16),
        "ty::Uint::U32" => TypeInfo::UnsignedInt(32),
        "ty::Uint::U64" => TypeInfo::UnsignedInt(64),
        "ty::Uint::Usize" => TypeInfo::UnsignedInt(64),
        "ty::Int::I8" => TypeInfo::SignedInt(8),
        "ty::Int::I16" => TypeInfo::SignedInt(16),
        "ty::Int::I32" => TypeInfo::SignedInt(32),
        "ty::Int::I64" => TypeInfo::SignedInt(64),
        "ty::Int::Isize" => TypeInfo::SignedInt(64),
        _ => TypeInfo::Opaque {
            name: ty_name.into(),
            size_bytes: 0,
        },
    };

    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };

    (ty, mutability, Nullability::NonNull)
}

fn parse_mir_type(ty_val: &Value) -> Option<TypeInfo> {
    let ty_name = ty_val.as_str()?;
    let (ty, _, _) = parse_rust_type_string(ty_name, false);
    Some(ty)
}
