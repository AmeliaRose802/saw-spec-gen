//! Generate Cryptol type constraint functions alongside SAW specs.
//!
//! Produces .cry files with predicates that constrain types to valid
//! ranges (e.g., enum discriminant bounds, option tag validity).

use crate::constraints::*;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Emit Cryptol constraint files from function info.
pub fn emit_cryptol_constraints(functions: &[FunctionInfo], output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory {}", output_dir.display()))?;

    let filepath = output_dir.join("auto_constraints.cry");
    let content = generate_cryptol_predicates(functions);
    fs::write(&filepath, content)
        .with_context(|| format!("Failed to write {}", filepath.display()))?;

    Ok(())
}

/// Generate Cryptol constraint predicates from FunctionInfo types.
pub fn generate_cryptol_predicates(functions: &[FunctionInfo]) -> String {
    let mut out = String::new();
    let mut emitted = std::collections::HashSet::new();

    out.push_str("// Auto-generated Cryptol type constraints\n");
    out.push_str("module AutoConstraints where\n\n");

    for func in functions {
        for param in &func.params {
            if let Some(pred) = generate_type_predicate(&param.ty, &mut emitted) {
                out.push_str(&pred);
                out.push('\n');
            }
        }
        if let Some(pred) = generate_type_predicate(&func.return_type, &mut emitted) {
            out.push_str(&pred);
            out.push('\n');
        }
    }

    out
}

fn generate_type_predicate(
    ty: &TypeInfo,
    emitted: &mut std::collections::HashSet<String>,
) -> Option<String> {
    match ty {
        TypeInfo::Enum {
            name,
            variants,
            discriminant_bits,
        } => {
            let fn_name = format!("valid_{}_disc", sanitize_cry_name(name));
            if !emitted.insert(fn_name.clone()) {
                return None;
            }
            let max = variants.len() as u64 - 1;
            let mut out = String::new();
            out.push_str(&format!("// Enum: {name} ({} variants)\n", variants.len()));
            for (i, v) in variants.iter().enumerate() {
                out.push_str(&format!("//   {i} = {v}\n"));
            }
            out.push_str(&format!(
                "{fn_name} : [{bits}] -> Bit\n",
                bits = discriminant_bits,
            ));
            out.push_str(&format!("{fn_name} x = x <= {max}\n"));
            Some(out)
        }
        TypeInfo::Option(inner) => {
            let inner_name = type_short_name(inner);
            let fn_name = format!("valid_option_{}_disc", sanitize_cry_name(&inner_name));
            if !emitted.insert(fn_name.clone()) {
                return None;
            }
            let mut out = String::new();
            out.push_str(&format!("// Option<{inner_name}>\n"));
            out.push_str(&format!("{fn_name} : [8] -> Bit\n"));
            out.push_str(&format!("{fn_name} x = x <= 1\n"));
            Some(out)
        }
        TypeInfo::Result(ok, err) => {
            let ok_name = type_short_name(ok);
            let err_name = type_short_name(err);
            let fn_name = format!(
                "valid_result_{}_{}_disc",
                sanitize_cry_name(&ok_name),
                sanitize_cry_name(&err_name)
            );
            if !emitted.insert(fn_name.clone()) {
                return None;
            }
            let mut out = String::new();
            out.push_str(&format!("// Result<{ok_name}, {err_name}>\n"));
            out.push_str(&format!("{fn_name} : [8] -> Bit\n"));
            out.push_str(&format!("{fn_name} x = x <= 1\n"));
            Some(out)
        }
        TypeInfo::Bool => {
            if !emitted.insert("valid_bool".into()) {
                return None;
            }
            let mut out = String::new();
            out.push_str("// Boolean constraint\n");
            out.push_str("valid_bool : [1] -> Bit\n");
            out.push_str("valid_bool x = x <= 1\n");
            Some(out)
        }
        TypeInfo::Pointer(inner) => generate_type_predicate(inner, emitted),
        _ => None,
    }
}

fn type_short_name(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Bool => "bool".into(),
        TypeInfo::SignedInt(bits) => format!("i{bits}"),
        TypeInfo::UnsignedInt(bits) => format!("u{bits}"),
        TypeInfo::Struct { name, .. } => name.clone(),
        TypeInfo::Enum { name, .. } => name.clone(),
        TypeInfo::Opaque { name, .. } => name.clone(),
        TypeInfo::Void => "void".into(),
        TypeInfo::ByteArray(n) => format!("bytes_{n}"),
        TypeInfo::Pointer(inner) => format!("ptr_{}", type_short_name(inner)),
        TypeInfo::Option(inner) => format!("opt_{}", type_short_name(inner)),
        TypeInfo::Result(ok, err) => {
            format!("res_{}_{}", type_short_name(ok), type_short_name(err))
        }
    }
}

fn sanitize_cry_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enum_predicate() {
        let mut emitted = std::collections::HashSet::new();
        let ty = TypeInfo::Enum {
            name: "LatchState".into(),
            variants: vec!["Unlatched".into(), "Latched".into()],
            discriminant_bits: 64,
        };
        let pred = generate_type_predicate(&ty, &mut emitted).unwrap();
        assert!(pred.contains("valid_LatchState_disc"));
        assert!(pred.contains("[64] -> Bit"));
        assert!(pred.contains("<= 1"));
    }

    #[test]
    fn test_option_predicate() {
        let mut emitted = std::collections::HashSet::new();
        let ty = TypeInfo::Option(Box::new(TypeInfo::UnsignedInt(32)));
        let pred = generate_type_predicate(&ty, &mut emitted).unwrap();
        assert!(pred.contains("valid_option_u32_disc"));
        assert!(pred.contains("[8] -> Bit"));
        assert!(pred.contains("<= 1"));
    }

    #[test]
    fn test_result_predicate() {
        let mut emitted = std::collections::HashSet::new();
        let ty = TypeInfo::Result(
            Box::new(TypeInfo::UnsignedInt(32)),
            Box::new(TypeInfo::SignedInt(32)),
        );
        let pred = generate_type_predicate(&ty, &mut emitted).unwrap();
        assert!(pred.contains("valid_result_u32_i32_disc"));
    }

    #[test]
    fn test_bool_predicate() {
        let mut emitted = std::collections::HashSet::new();
        let pred = generate_type_predicate(&TypeInfo::Bool, &mut emitted).unwrap();
        assert!(pred.contains("valid_bool"));
    }

    #[test]
    fn test_no_duplicate_predicates() {
        let mut emitted = std::collections::HashSet::new();
        let ty = TypeInfo::Bool;
        assert!(generate_type_predicate(&ty, &mut emitted).is_some());
        assert!(generate_type_predicate(&ty, &mut emitted).is_none());
    }

    #[test]
    fn test_generate_cryptol_predicates() {
        let funcs = vec![FunctionInfo {
            name: "check".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "state".into(),
                ty: TypeInfo::Enum {
                    name: "Status".into(),
                    variants: vec!["Ok".into(), "Err".into()],
                    discriminant_bits: 32,
                },
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Bool,
            can_throw: false,
            annotations: vec![],
        }];
        let output = generate_cryptol_predicates(&funcs);
        assert!(output.contains("valid_Status_disc"));
        assert!(output.contains("valid_bool"));
    }

    #[test]
    fn test_emit_cryptol_constraints_creates_file() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_cryptol");
        let _ = fs::remove_dir_all(&dir);

        let funcs = vec![FunctionInfo {
            name: "test".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "x".into(),
                ty: TypeInfo::Enum {
                    name: "Status".into(),
                    variants: vec!["Ok".into(), "Err".into()],
                    discriminant_bits: 32,
                },
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Bool,
            can_throw: false,
            annotations: vec![],
        }];
        emit_cryptol_constraints(&funcs, &dir).unwrap();
        assert!(dir.join("auto_constraints.cry").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
