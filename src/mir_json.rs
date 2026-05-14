//! Parse mir-json output and extract function signatures with Rust type info.
//!
//! mir-json serializes Rust MIR to JSON, preserving full type information
//! including enum variants, reference mutability, lifetimes, etc.

use crate::constraints::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Maximum file size we'll attempt to load into memory (100 MB).
const MAX_MIR_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Parse a mir-json output file.
pub fn parse_mir(path: &Path) -> Result<Value> {
    // Check file size before loading to avoid OOM on huge MIR dumps
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_MIR_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "MIR file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use --filter to process only specific functions\n\
             \n  2. Build with a smaller crate scope (e.g. single lib target)\n\
             \n  3. Pre-filter with: jq '.fns |= map(select(.name | test(\"pattern\")))' large.json > filtered.json",
            path.display(),
            size_mb,
            MAX_MIR_FILE_SIZE / (1024 * 1024),
        );
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mir: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
    Ok(mir)
}

/// ADT (algebraic data type) info parsed from mir-json.
struct AdtInfo {
    name: String,
    ty: TypeInfo,
}

/// Build a type resolution table from the "adts" array in mir-json.
fn build_adt_map(mir: &Value) -> HashMap<String, TypeInfo> {
    let mut adt_map = HashMap::new();

    if let Some(adts) = mir.get("adts").and_then(|v| v.as_array()) {
        for adt in adts {
            if let Some(info) = parse_adt_def(adt) {
                adt_map.insert(info.name, info.ty);
            }
        }
    }

    adt_map
}

fn parse_adt_def(adt: &Value) -> Option<AdtInfo> {
    let name = adt.get("name")?.as_str()?.to_string();
    let kind = adt
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("struct");
    let variants = adt.get("variants").and_then(|v| v.as_array());

    match kind {
        "enum" => {
            let variant_names: Vec<String> = variants
                .map(|vs| {
                    vs.iter()
                        .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let disc_bits = adt
                .get("discriminant_bits")
                .and_then(|v| v.as_u64())
                .unwrap_or(64) as u32;
            Some(AdtInfo {
                name,
                ty: TypeInfo::Enum {
                    name: String::new(), // Will be set from the outer name
                    variants: variant_names,
                    discriminant_bits: disc_bits,
                },
            })
        }
        "struct" | "union" => {
            let fields: Vec<(String, TypeInfo)> = variants
                .and_then(|vs| vs.first())
                .and_then(|v| v.get("fields"))
                .and_then(|v| v.as_array())
                .map(|fs| {
                    fs.iter()
                        .filter_map(|f| {
                            let fname = f.get("name")?.as_str()?.to_string();
                            let fty = f.get("ty").and_then(|t| t.as_str()).unwrap_or("()");
                            let (ty, _, _) = parse_rust_type_string(fty, false, &HashMap::new());
                            Some((fname, ty))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let size = adt
                .get("size")
                .and_then(|v| v.as_u64())
                .map(|s| s as usize);
            Some(AdtInfo {
                name,
                ty: TypeInfo::Struct {
                    name: String::new(),
                    size_bytes: size,
                    fields,
                },
            })
        }
        _ => None,
    }
}

/// Extract function info from MIR JSON.
pub fn extract_functions(mir: &Value, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let adt_map = build_adt_map(mir);
    let mut functions = Vec::new();

    if let Some(fns) = mir.get("fns").and_then(|v| v.as_array()) {
        for func_val in fns {
            if let Some(func) = parse_mir_function(func_val, &adt_map) {
                let matches = filter.map(|f| func.name.contains(f)).unwrap_or(true);
                if matches {
                    functions.push(func);
                }
            }
        }
    }

    Ok(functions)
}

fn parse_mir_function(func: &Value, adt_map: &HashMap<String, TypeInfo>) -> Option<FunctionInfo> {
    let name = func.get("name")?.as_str()?.to_string();
    let _abi = func
        .get("abi")
        .and_then(|v| v.get("kind"))
        .and_then(|v| v.as_str());

    // Check for async/coroutine indicators
    let is_async = func
        .get("is_async")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || func
            .get("return_ty")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.contains("GenFuture")
                    || s.contains("Coroutine")
                    || s.contains("impl Future")
                    || s.contains("Poll")
            })
            .unwrap_or(false);

    let mut params = Vec::new();

    if let Some(args) = func.get("args").and_then(|v| v.as_array()) {
        for arg in args {
            if let Some(param) = parse_mir_param(arg, adt_map) {
                params.push(param);
            }
        }
    }

    let return_type = func
        .get("return_ty")
        .and_then(|v| parse_mir_type(v, adt_map))
        .unwrap_or(TypeInfo::Void);

    // For async functions, wrap return type in an opaque coroutine type
    let return_type = if is_async {
        TypeInfo::Opaque {
            name: format!("async_coroutine<{}>", name),
            size_bytes: 0,
        }
    } else {
        return_type
    };

    let mut annotations = Vec::new();

    // Check for function attributes
    if let Some(attrs) = func.get("attrs").and_then(|v| v.as_array()) {
        for attr in attrs {
            if let Some(attr_name) = attr.as_str() {
                match attr_name {
                    "nounwind" | "noexcept" => annotations.push(Annotation::NoThrow),
                    _ => {}
                }
            }
        }
    }

    Some(FunctionInfo {
        name,
        mangled_name: None,
        params,
        return_type,
        can_throw: false, // Rust doesn't have exceptions
        annotations,
    })
}

fn parse_mir_param(arg: &Value, adt_map: &HashMap<String, TypeInfo>) -> Option<ParamInfo> {
    let name = arg.get("name")?.as_str()?.to_string();
    let ty_name = arg.get("ty")?.as_str()?;
    let is_mut = arg
        .get("mut")
        .and_then(|v| v.get("kind"))
        .and_then(|v| v.as_str())
        == Some("Mut");

    let (ty, mutability, nullable) = parse_rust_type_string(ty_name, is_mut, adt_map);

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
    adt_map: &HashMap<String, TypeInfo>,
) -> (TypeInfo, Mutability, Nullability) {
    // Rust references: &T is readonly, &mut T is mutable, both are non-null
    if ty_name.starts_with("ty::Ref") {
        let mutability = if is_mut {
            Mutability::Mutable
        } else {
            Mutability::Readonly
        };
        // Try to extract the inner type name
        let inner_ty = extract_ref_inner_type(ty_name, adt_map);
        return (
            TypeInfo::Pointer(Box::new(inner_ty)),
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
        let inner_ty = extract_ref_inner_type(ty_name, adt_map);
        return (
            TypeInfo::Pointer(Box::new(inner_ty)),
            mutability,
            Nullability::Nullable,
        );
    }

    // Option<T>
    if ty_name.starts_with("Option<") || ty_name.contains("::Option<") {
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
        return (TypeInfo::Option(Box::new(inner_ty)), mutability, Nullability::NonNull);
    }

    // Result<T, E>
    if ty_name.starts_with("Result<") || ty_name.contains("::Result<") {
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
        return (
            TypeInfo::Result(Box::new(ok_ty), Box::new(err_ty)),
            mutability,
            Nullability::NonNull,
        );
    }

    // Common Rust types
    let ty = match ty_name {
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
        "()" | "ty::Tuple::unit" => TypeInfo::Void,
        other => {
            // Check ADT map
            if let Some(adt_ty) = adt_map.get(other) {
                match adt_ty {
                    TypeInfo::Enum {
                        variants,
                        discriminant_bits,
                        ..
                    } => TypeInfo::Enum {
                        name: other.to_string(),
                        variants: variants.clone(),
                        discriminant_bits: *discriminant_bits,
                    },
                    TypeInfo::Struct {
                        size_bytes, fields, ..
                    } => TypeInfo::Struct {
                        name: other.to_string(),
                        size_bytes: *size_bytes,
                        fields: fields.clone(),
                    },
                    _ => adt_ty.clone(),
                }
            } else {
                TypeInfo::Opaque {
                    name: other.into(),
                    size_bytes: 0,
                }
            }
        }
    };

    let mutability = if is_mut {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };

    (ty, mutability, Nullability::NonNull)
}

fn extract_ref_inner_type(ty_name: &str, adt_map: &HashMap<String, TypeInfo>) -> TypeInfo {
    // Try to extract inner type from reference/pointer type string
    // e.g., "ty::Ref<'_, u8, Shared>" -> u8
    if let Some(start) = ty_name.find('<') {
        let inner = &ty_name[start + 1..];
        if let Some(end) = inner.rfind('>') {
            let parts: Vec<&str> = inner[..end].split(", ").collect();
            // For ty::Ref, format is often <'lifetime, Type, Mutability>
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

fn parse_mir_type(ty_val: &Value, adt_map: &HashMap<String, TypeInfo>) -> Option<TypeInfo> {
    let ty_name = ty_val.as_str()?;
    let (ty, _, _) = parse_rust_type_string(ty_name, false, adt_map);
    Some(ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_simple_function() {
        let mir = json!({
            "fns": [{
                "name": "add",
                "args": [
                    { "name": "a", "ty": "ty::Int::I32", "mut": { "kind": "Not" } },
                    { "name": "b", "ty": "ty::Int::I32", "mut": { "kind": "Not" } }
                ],
                "return_ty": "ty::Int::I32"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
        assert_eq!(funcs[0].params[0].ty, TypeInfo::SignedInt(32));
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
        assert!(!funcs[0].can_throw);
    }

    #[test]
    fn test_parse_ref_params() {
        let mir = json!({
            "fns": [{
                "name": "process",
                "args": [
                    { "name": "input", "ty": "ty::Ref", "mut": { "kind": "Not" } },
                    { "name": "output", "ty": "ty::Ref", "mut": { "kind": "Mut" } }
                ],
                "return_ty": "()"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
        assert_eq!(funcs[0].params[0].nullable, Nullability::NonNull);
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
        assert_eq!(funcs[0].params[1].mutability, Mutability::Mutable);
    }

    #[test]
    fn test_parse_raw_pointer() {
        let mir = json!({
            "fns": [{
                "name": "unsafe_fn",
                "args": [{
                    "name": "ptr",
                    "ty": "RawPtr",
                    "mut": { "kind": "Not" }
                }],
                "return_ty": "()"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(funcs[0].params[0].nullable, Nullability::Nullable);
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
    }

    #[test]
    fn test_parse_enum_from_adts() {
        let mir = json!({
            "adts": [{
                "name": "LatchState",
                "kind": "enum",
                "variants": [
                    { "name": "Unlatched", "fields": [] },
                    { "name": "Latched", "fields": [] }
                ],
                "discriminant_bits": 64
            }],
            "fns": [{
                "name": "check_latch",
                "args": [{
                    "name": "state",
                    "ty": "LatchState",
                    "mut": { "kind": "Not" }
                }],
                "return_ty": "ty::Bool"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        if let TypeInfo::Enum {
            name,
            variants,
            discriminant_bits,
        } = &funcs[0].params[0].ty
        {
            assert_eq!(name, "LatchState");
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[0], "Unlatched");
            assert_eq!(variants[1], "Latched");
            assert_eq!(*discriminant_bits, 64);
        } else {
            panic!(
                "Expected Enum type, got {:?}",
                funcs[0].params[0].ty
            );
        }
    }

    #[test]
    fn test_parse_struct_from_adts() {
        let mir = json!({
            "adts": [{
                "name": "MetadataHeader",
                "kind": "struct",
                "size": 24,
                "variants": [{
                    "name": "MetadataHeader",
                    "fields": [
                        { "name": "version", "ty": "ty::Uint::U32" },
                        { "name": "length", "ty": "ty::Uint::U64" },
                        { "name": "flags", "ty": "ty::Uint::U32" }
                    ]
                }]
            }],
            "fns": [{
                "name": "validate_header",
                "args": [{
                    "name": "header",
                    "ty": "MetadataHeader",
                    "mut": { "kind": "Not" }
                }],
                "return_ty": "ty::Bool"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        if let TypeInfo::Struct {
            name,
            size_bytes,
            fields,
        } = &funcs[0].params[0].ty
        {
            assert_eq!(name, "MetadataHeader");
            assert_eq!(*size_bytes, Some(24));
            assert_eq!(fields.len(), 3);
            assert_eq!(fields[0].0, "version");
        } else {
            panic!(
                "Expected Struct type, got {:?}",
                funcs[0].params[0].ty
            );
        }
    }

    #[test]
    fn test_parse_option_type() {
        let mir = json!({
            "fns": [{
                "name": "maybe_value",
                "args": [{
                    "name": "opt",
                    "ty": "Option<ty::Uint::U32>",
                    "mut": { "kind": "Not" }
                }],
                "return_ty": "()"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        if let TypeInfo::Option(inner) = &funcs[0].params[0].ty {
            assert_eq!(**inner, TypeInfo::UnsignedInt(32));
        } else {
            panic!(
                "Expected Option type, got {:?}",
                funcs[0].params[0].ty
            );
        }
    }

    #[test]
    fn test_parse_result_type() {
        let mir = json!({
            "fns": [{
                "name": "try_parse",
                "args": [],
                "return_ty": "Result<ty::Uint::U32, ty::Int::I32>"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        if let TypeInfo::Result(ok, err) = &funcs[0].return_type {
            assert_eq!(**ok, TypeInfo::UnsignedInt(32));
            assert_eq!(**err, TypeInfo::SignedInt(32));
        } else {
            panic!(
                "Expected Result type, got {:?}",
                funcs[0].return_type
            );
        }
    }

    #[test]
    fn test_parse_async_function() {
        let mir = json!({
            "fns": [{
                "name": "async_fetch",
                "is_async": true,
                "args": [],
                "return_ty": "impl Future"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        if let TypeInfo::Opaque { name, .. } = &funcs[0].return_type {
            assert!(name.contains("async_coroutine"));
        } else {
            panic!(
                "Expected Opaque coroutine type, got {:?}",
                funcs[0].return_type
            );
        }
    }

    #[test]
    fn test_parse_coroutine_return_type() {
        let mir = json!({
            "fns": [{
                "name": "poll_fn",
                "args": [],
                "return_ty": "Poll"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        // Poll return type on non-async function should trigger coroutine detection
        assert!(matches!(funcs[0].return_type, TypeInfo::Opaque { .. }));
    }

    #[test]
    fn test_filter_functions() {
        let mir = json!({
            "fns": [
                {
                    "name": "latch_local",
                    "args": [],
                    "return_ty": "()"
                },
                {
                    "name": "validate_header",
                    "args": [],
                    "return_ty": "()"
                }
            ]
        });
        let funcs = extract_functions(&mir, Some("latch")).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "latch_local");
    }

    #[test]
    fn test_parse_bool_types() {
        let mir = json!({
            "fns": [{
                "name": "check",
                "args": [{
                    "name": "flag",
                    "ty": "ty::Bool",
                    "mut": { "kind": "Not" }
                }],
                "return_ty": "ty::Bool"
            }]
        });
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(funcs[0].params[0].ty, TypeInfo::Bool);
        assert_eq!(funcs[0].return_type, TypeInfo::Bool);
    }
}
