//! Parse clang -ast-dump=json output and extract function signatures.
//!
//! Usage:
//!   clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json
//!   saw-spec-gen from-clang-ast --input ast.json --output specs/

use crate::constraints::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Parse a clang AST JSON dump.
pub fn parse_ast(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let ast: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
    Ok(ast)
}

/// Extract function info from the clang AST.
pub fn extract_functions(ast: &Value, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let mut functions = Vec::new();
    collect_functions(ast, &mut functions, filter);
    Ok(functions)
}

fn collect_functions(node: &Value, out: &mut Vec<FunctionInfo>, filter: Option<&str>) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        if kind == "FunctionDecl" || kind == "CXXMethodDecl" {
            if let Some(func) = parse_function_decl(node, kind == "CXXMethodDecl") {
                let matches = filter
                    .map(|f| func.name.contains(f))
                    .unwrap_or(true);
                if matches {
                    out.push(func);
                }
            }
        }
    }

    // Recurse into child nodes
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_functions(child, out, filter);
        }
    }
}

fn parse_function_decl(node: &Value, is_method: bool) -> Option<FunctionInfo> {
    let name = node.get("name")?.as_str()?.to_string();
    let mangled = node.get("mangledName").and_then(|v| v.as_str()).map(String::from);
    let qual_type = node.get("type")?.get("qualType")?.as_str()?;

    let is_const_method = is_method && qual_type.contains("const");
    let is_noexcept = qual_type.contains("noexcept");

    let mut params = Vec::new();

    // If this is a const method, add implicit 'this' as readonly
    if is_method {
        let parent_name = node
            .get("parentDeclContextId")
            .and_then(|v| v.as_str())
            .unwrap_or("Self");
        params.push(ParamInfo {
            name: "this".into(),
            ty: TypeInfo::Opaque {
                name: parent_name.into(),
                size_bytes: 0, // Unknown at AST level
            },
            mutability: if is_const_method {
                Mutability::Readonly
            } else {
                Mutability::Mutable
            },
            nullable: Nullability::NonNull,
            annotations: Vec::new(),
        });
    }

    // Parse explicit parameters
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            if child.get("kind").and_then(|v| v.as_str()) == Some("ParmVarDecl") {
                if let Some(param) = parse_param(child) {
                    params.push(param);
                }
            }
        }
    }

    let return_type = parse_return_type(qual_type);

    Some(FunctionInfo {
        name,
        mangled_name: mangled,
        params,
        return_type,
        can_throw: !is_noexcept,
        annotations: Vec::new(),
    })
}

fn parse_param(node: &Value) -> Option<ParamInfo> {
    let name = node
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();
    let qual_type = node.get("type")?.get("qualType")?.as_str()?;

    let is_const = qual_type.contains("const");
    let is_ref = qual_type.contains('&');
    let is_ptr = qual_type.contains('*');

    let mutability = if is_const && (is_ref || is_ptr) {
        Mutability::Readonly
    } else if is_ref || is_ptr {
        Mutability::Mutable
    } else {
        Mutability::Readonly // Pass-by-value is effectively readonly
    };

    let nullable = if is_ref {
        Nullability::NonNull // References can't be null
    } else if is_ptr {
        Nullability::Nullable
    } else {
        Nullability::NonNull
    };

    // Check for SAL annotations in attributes
    let mut annotations = Vec::new();
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for attr in inner {
            if let Some(ann) = parse_sal_annotation(attr) {
                annotations.push(ann);
            }
        }
    }

    // SAL overrides: _In_ means readonly, _Out_ means writeonly
    let mutability = annotations.iter().fold(mutability, |m, ann| match ann {
        Annotation::InReads(_) => Mutability::Readonly,
        Annotation::OutWrites(_) => Mutability::WriteOnly,
        _ => m,
    });

    let ty = parse_cpp_type(qual_type);

    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations,
    })
}

fn parse_sal_annotation(attr: &Value) -> Option<Annotation> {
    let kind = attr.get("kind")?.as_str()?;
    if kind != "AnnotateAttr" {
        return None;
    }
    let value = attr.get("value")?.as_str()?;
    match value {
        v if v.starts_with("_In_reads_(") => {
            let n: usize = v
                .trim_start_matches("_In_reads_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::InReads(n))
        }
        v if v.starts_with("_Out_writes_(") => {
            let n: usize = v
                .trim_start_matches("_Out_writes_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::OutWrites(n))
        }
        "_In_" => Some(Annotation::InReads(0)),
        "_Out_" => Some(Annotation::OutWrites(0)),
        "_Must_inspect_result_" => Some(Annotation::MustInspectResult),
        _ => Some(Annotation::Custom(value.into())),
    }
}

fn parse_return_type(qual_type: &str) -> TypeInfo {
    // Extract return type from function type string "RetType (ParamTypes)"
    let ret = qual_type.split('(').next().unwrap_or("void").trim();
    parse_cpp_type(ret)
}

fn parse_cpp_type(qual_type: &str) -> TypeInfo {
    let t = qual_type.trim();
    let t = t.trim_start_matches("const ");
    let t = t.trim_end_matches('&').trim_end_matches('*').trim();

    match t {
        "bool" => TypeInfo::Bool,
        "int" | "int32_t" | "long" => TypeInfo::SignedInt(32),
        "long long" | "int64_t" => TypeInfo::SignedInt(64),
        "short" | "int16_t" => TypeInfo::SignedInt(16),
        "char" | "int8_t" => TypeInfo::SignedInt(8),
        "unsigned int" | "uint32_t" | "unsigned long" | "DWORD" => TypeInfo::UnsignedInt(32),
        "unsigned long long" | "uint64_t" | "size_t" => TypeInfo::UnsignedInt(64),
        "unsigned short" | "uint16_t" | "WORD" => TypeInfo::UnsignedInt(16),
        "unsigned char" | "uint8_t" | "BYTE" => TypeInfo::UnsignedInt(8),
        "void" => TypeInfo::Void,
        other => TypeInfo::Opaque {
            name: other.to_string(),
            size_bytes: 0,
        },
    }
}
