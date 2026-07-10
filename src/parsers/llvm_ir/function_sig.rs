//! Function-signature parsing: `declare`/`define` lines → [`FunctionInfo`].

use super::params::parse_ir_params_resolved;
use super::struct_types::{parse_struct_types, IrStructDef};
use super::tokens::{find_matching_paren, update_paren_depth};
use super::type_parser::parse_ir_type_resolved;
use crate::constraints::{Annotation, FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo};
use anyhow::Result;
use std::collections::HashMap;

/// Extract every function declaration/definition from LLVM IR text,
/// optionally filtered by name substring.
pub fn extract_functions(ir: &str, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let struct_map = parse_struct_types(ir);
    let mut functions = Vec::new();
    for line in ir.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("declare ") && !trimmed.starts_with("define ") {
            continue;
        }
        let Some(func) = parse_ir_function(trimmed, &struct_map) else {
            continue;
        };
        if filter.map(|f| func.name.contains(f)).unwrap_or(true) {
            functions.push(func);
        }
    }
    Ok(functions)
}

/// Parse a single `declare`/`define` line.
pub fn parse_ir_function(
    line: &str,
    struct_map: &HashMap<String, IrStructDef>,
) -> Option<FunctionInfo> {
    let is_define = line.starts_with("define ");
    let has_nounwind = line.contains("nounwind");

    // `declare/define [linkage] [visibility] [ret_type] @name(params) [attrs]`
    let at_pos = line.find('@')?;
    let paren_open = line[at_pos..].find('(')? + at_pos;
    let paren_close = find_matching_paren(line, paren_open)?;

    let name = line[at_pos + 1..paren_open].to_string();
    let params_str = &line[paren_open + 1..paren_close];

    let ret_str = extract_return_type(line, at_pos);
    let return_type = parse_ir_type_resolved(&ret_str, struct_map);
    let params = parse_ir_params_resolved(params_str, struct_map);
    let (sret_param, regular_params, effective_return) = extract_sret(&params, &return_type);

    let mut final_params = Vec::new();
    if let Some(sp) = sret_param {
        final_params.push(sp);
    }
    final_params.extend(regular_params);

    let attrs_str = &line[paren_close + 1..];
    let mut annotations = Vec::new();
    if has_nounwind || attrs_str.contains("nounwind") {
        annotations.push(Annotation::NoThrow);
    }

    Some(FunctionInfo {
        name,
        mangled_name: None,
        params: final_params,
        return_type: effective_return,
        can_throw: !has_nounwind,
        is_virtual: false,
        has_body: is_define,
        is_system: false,
        annotations,
        referenced_globals: vec![],
        called_functions: vec![],
    })
}

/// Detect and rewrite an `sret` parameter into an output-parameter
/// `WriteOnly` slot, with the original return type promoted to the
/// inner struct so [`crate::constraints::derive_return_constraint`]
/// can emit the right postcondition.
pub fn extract_sret(
    params: &[ParamInfo],
    original_return: &TypeInfo,
) -> (Option<ParamInfo>, Vec<ParamInfo>, TypeInfo) {
    let Some((idx, sret_param)) = params.iter().enumerate().find(|(_, p)| {
        p.annotations
            .iter()
            .any(|a| matches!(a, Annotation::Custom(s) if s == "sret"))
    }) else {
        return (None, params.to_vec(), original_return.clone());
    };

    // Keep the sret annotation on the extracted param so that callers can
    // distinguish a genuine sret slot from other WriteOnly parameters (e.g.
    // params with a plain LLVM `writeonly` attribute).
    let out_param = ParamInfo {
        name: sret_param.name.clone(),
        ty: sret_param.ty.clone(),
        mutability: Mutability::WriteOnly,
        nullable: Nullability::NonNull,
        annotations: sret_param.annotations.clone(),
    };
    let remaining: Vec<ParamInfo> = params
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, p)| p.clone())
        .collect();
    let sret_return = match &sret_param.ty {
        TypeInfo::Pointer(inner) => *inner.clone(),
        other => other.clone(),
    };
    (Some(out_param), remaining, sret_return)
}

/// Extract the return-type substring from a `declare`/`define` line
/// (the chunk between the keyword + linkage/etc. attributes and the
/// `@name` token).
pub fn extract_return_type(line: &str, at_pos: usize) -> String {
    let after_keyword = if line.starts_with("declare ") {
        &line[8..at_pos]
    } else if line.starts_with("define ") {
        &line[7..at_pos]
    } else {
        return "void".to_string();
    };
    let parts: Vec<&str> = after_keyword.split_whitespace().collect();
    let mut type_parts: Vec<&str> = Vec::new();
    let mut skip_depth: i32 = 0;
    for p in &parts {
        if skip_depth > 0 {
            skip_depth = update_paren_depth(p, skip_depth);
            continue;
        }
        if LINKAGE_KEYWORDS.contains(p) || p.starts_with('#') {
            continue;
        }
        if PAREN_ATTR_PREFIXES
            .iter()
            .any(|prefix| p.starts_with(prefix))
        {
            skip_depth = update_paren_depth(p, 0).max(0);
            continue;
        }
        type_parts.push(p);
    }
    type_parts.join(" ").trim().to_string()
}

/// Linkage / visibility / calling-convention keywords that appear
/// between `declare`/`define` and the return type, and need to be
/// skipped when assembling the return-type substring.
const LINKAGE_KEYWORDS: &[&str] = &[
    "alwaysinline",
    "appending",
    "available_externally",
    "ccc",
    "coldcc",
    "common",
    "dead_on_unwind",
    "default",
    "dllexport",
    "dllimport",
    "dso_local",
    "dso_preemptable",
    "extern_weak",
    "external",
    "fastcc",
    "hidden",
    "internal",
    "linkonce",
    "linkonce_odr",
    "mustprogress",
    "noinline",
    "noreturn",
    "noundef",
    "nounwind",
    "optnone",
    "optsize",
    "private",
    "protected",
    "signext",
    "swiftcc",
    "uwtable",
    "weak",
    "weak_odr",
    "willreturn",
    "win64cc",
    "writable",
    "x86_stdcallcc",
    "zeroext",
];

/// Parenthesized attribute groups that may appear in front of the
/// return type and span multiple whitespace-separated tokens.
const PAREN_ATTR_PREFIXES: &[&str] = &["align(", "captures(", "memory(", "range("];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_declare_strlen_like_signature() {
        let ir = "declare i32 @strlen(i8* nocapture readonly) nounwind\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "strlen");
        assert!(!funcs[0].can_throw);
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
    }

    #[test]
    fn parse_define_add_signature() {
        let ir = "define dso_local i32 @add(i32 %a, i32 %b) { ret i32 0 }\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
        assert!(funcs[0].has_body);
    }

    #[test]
    fn parse_void_function() {
        let ir = "declare void @noop()\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs[0].return_type, TypeInfo::Void);
    }

    #[test]
    fn nounwind_emits_nothrow_annotation() {
        let ir = "declare i32 @safe() nounwind\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert!(funcs[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoThrow)));
    }

    #[test]
    fn ptr_type_param_is_wrapped() {
        let ir = "declare void @write_buf(ptr writeonly nonnull %dst)\n";
        let funcs = extract_functions(ir, None).unwrap();
        let p = &funcs[0].params[0];
        assert!(matches!(p.ty, TypeInfo::Pointer(_)));
        assert_eq!(p.mutability, Mutability::WriteOnly);
        assert_eq!(p.nullable, Nullability::NonNull);
    }

    #[test]
    fn filter_limits_results_by_substring() {
        let ir = "declare void @aaa()\ndeclare void @bbb()\ndeclare void @abc()\n";
        let funcs = extract_functions(ir, Some("aa")).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "aaa");
    }

    #[test]
    fn extract_return_type_skips_linkage_attrs() {
        let line = "define dso_local nounwind i32 @foo() { ret i32 0 }";
        let at = line.find('@').unwrap();
        assert_eq!(extract_return_type(line, at), "i32");
    }

    #[test]
    fn extract_return_type_skips_paren_groups() {
        let line = "define range(i32 0, 4) i32 @foo() { ret i32 0 }";
        let at = line.find('@').unwrap();
        assert_eq!(extract_return_type(line, at), "i32");
    }

    #[test]
    fn extract_sret_returns_none_for_normal_signatures() {
        let params = vec![ParamInfo {
            name: "x".into(),
            ty: TypeInfo::SignedInt(32),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }];
        let (out, rest, ret) = extract_sret(&params, &TypeInfo::Void);
        assert!(out.is_none());
        assert_eq!(rest.len(), 1);
        assert_eq!(ret, TypeInfo::Void);
    }

    #[test]
    fn extract_sret_pulls_out_marked_param() {
        let sret_param = ParamInfo {
            name: "retval".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: "BigResult".into(),
                size_bytes: Some(48),
                fields: vec![],
            })),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: vec![Annotation::Custom("sret".into())],
        };
        let regular = ParamInfo {
            name: "x".into(),
            ty: TypeInfo::SignedInt(32),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        };
        let (out, rest, ret) = extract_sret(&[sret_param, regular], &TypeInfo::Void);
        assert!(out.is_some());
        let out_param = out.as_ref().unwrap();
        assert_eq!(out_param.mutability, Mutability::WriteOnly);
        // Sret annotation is KEPT on the extracted param so downstream callers
        // can distinguish it from other WriteOnly parameters (e.g. writeonly
        // LLVM attribute on a non-sret pointer).
        assert!(out_param
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::Custom(s) if s == "sret")));
        assert!(matches!(ret, TypeInfo::Struct { .. }));
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].name, "x");
    }

    #[test]
    fn integration_sret_signature_returns_struct() {
        // declare with sret(%struct.Foo) — expect WriteOnly + struct return.
        let ir = "%struct.Big = type { i32, i32, i32, i32 }\n\
                  declare void @get_big(ptr sret(%struct.Big) %ret)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        let f = &funcs[0];
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].mutability, Mutability::WriteOnly);
        assert!(matches!(f.return_type, TypeInfo::Struct { .. }));
    }

    #[test]
    fn parses_multiple_functions_from_one_blob() {
        let ir = "\
declare void @one()
declare i32 @two(i32)
define i64 @three() { ret i64 0 }
";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 3);
        assert!(funcs[2].has_body);
    }
}
