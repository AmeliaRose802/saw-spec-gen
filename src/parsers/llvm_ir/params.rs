//! Parameter-list parsing: split → classify → assemble [`ParamInfo`].

use super::attrs::IrParamAttrs;
use super::struct_types::IrStructDef;
use super::tokens::split_ir_params;
use super::type_parser::parse_ir_type_resolved;
use crate::constraints::{Annotation, Mutability, Nullability, ParamInfo, TypeInfo};
use std::collections::HashMap;

/// Parse a comma-separated LLVM IR parameter list. Filters out the
/// `...` varargs marker and skips entries the per-param parser can't
/// make sense of.
pub fn parse_ir_params_resolved(
    params_str: &str,
    struct_map: &HashMap<String, IrStructDef>,
) -> Vec<ParamInfo> {
    if params_str.trim().is_empty() || params_str.trim() == "..." {
        return Vec::new();
    }
    let mut params = Vec::new();
    for (idx, param_str) in split_ir_params(params_str).into_iter().enumerate() {
        let trimmed = param_str.trim();
        if trimmed == "..." {
            continue;
        }
        if let Some(p) = parse_ir_param_resolved(trimmed, idx, struct_map) {
            params.push(p);
        }
    }
    params
}

/// Parse a single parameter declaration into a [`ParamInfo`].
///
/// Strategy: feed the whitespace-tokenised text through
/// [`IrParamAttrs::from_parts`] to extract every attribute we care
/// about (and synthesize a name), then map the resulting flags to
/// `Mutability` + `Nullability` + `Annotation`s.
pub fn parse_ir_param_resolved(
    param_str: &str,
    idx: usize,
    struct_map: &HashMap<String, IrStructDef>,
) -> Option<ParamInfo> {
    let parts: Vec<&str> = param_str.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    let attrs = IrParamAttrs::from_parts(&parts);
    if attrs.type_str.is_empty() {
        return None;
    }

    let is_ptr = attrs.type_str.contains('*') || attrs.type_str == "ptr";
    let inner_ty = parse_ir_type_resolved(&attrs.type_str, struct_map);
    let ty = if is_ptr {
        TypeInfo::Pointer(Box::new(inner_ty))
    } else {
        inner_ty
    };

    let mutability = derive_mutability(&attrs, is_ptr);
    let nullable = derive_nullability(&attrs, is_ptr);
    let annotations = build_annotations(&attrs);

    Some(ParamInfo {
        name: attrs.name.unwrap_or_else(|| format!("arg{idx}")),
        ty,
        mutability,
        nullable,
        annotations,
    })
}

/// `sret` and `writeonly` ⇒ `WriteOnly`; explicit `readonly` ⇒
/// `Readonly`; bare pointers ⇒ `Mutable`; everything else ⇒
/// `Readonly` (pass-by-value).
fn derive_mutability(a: &IrParamAttrs, is_ptr: bool) -> Mutability {
    if a.writeonly || a.sret {
        Mutability::WriteOnly
    } else if a.readonly {
        Mutability::Readonly
    } else if is_ptr {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    }
}

/// `nonnull`, `sret`, and non-pointer params are non-null; otherwise
/// pointer params are `Nullable`.
fn derive_nullability(a: &IrParamAttrs, is_ptr: bool) -> Nullability {
    if a.nonnull || a.sret || !is_ptr {
        Nullability::NonNull
    } else {
        Nullability::Nullable
    }
}

fn build_annotations(a: &IrParamAttrs) -> Vec<Annotation> {
    let mut out = Vec::new();
    if a.noalias {
        out.push(Annotation::NoAlias);
    }
    if a.nocapture {
        out.push(Annotation::NoCapture);
    }
    if let Some(n) = a.deref_size {
        out.push(Annotation::Dereferenceable(n));
    }
    if a.sret {
        out.push(Annotation::Custom("sret".into()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_pointer_param() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("i8* nocapture readonly", 0, &m).unwrap();
        assert_eq!(p.mutability, Mutability::Readonly);
        assert!(matches!(p.ty, TypeInfo::Pointer(_)));
        assert!(p
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoCapture)));
    }

    #[test]
    fn mutable_default_for_unannotated_pointer() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("i32* %buf", 0, &m).unwrap();
        assert_eq!(p.mutability, Mutability::Mutable);
        assert_eq!(p.name, "buf");
    }

    #[test]
    fn passes_value_param_marked_readonly() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("i32 %x", 0, &m).unwrap();
        assert_eq!(p.mutability, Mutability::Readonly);
        assert_eq!(p.nullable, Nullability::NonNull);
    }

    #[test]
    fn nonnull_pointer_is_non_null() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("ptr nonnull %p", 0, &m).unwrap();
        assert_eq!(p.nullable, Nullability::NonNull);
    }

    #[test]
    fn unannotated_pointer_is_nullable() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("ptr %p", 0, &m).unwrap();
        assert_eq!(p.nullable, Nullability::Nullable);
    }

    #[test]
    fn sret_param_is_writeonly_with_annotation() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("ptr sret(%struct.Foo) %retval", 0, &m).unwrap();
        assert_eq!(p.mutability, Mutability::WriteOnly);
        assert_eq!(p.nullable, Nullability::NonNull);
        assert!(p
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::Custom(s) if s == "sret")));
    }

    #[test]
    fn dereferenceable_emits_annotation() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("ptr dereferenceable(64) %p", 0, &m).unwrap();
        assert!(p
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::Dereferenceable(64))));
    }

    #[test]
    fn split_drops_varargs_marker() {
        let m = HashMap::new();
        let ps = parse_ir_params_resolved("i32 %x, ...", &m);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].name, "x");
    }

    #[test]
    fn empty_param_list() {
        let m = HashMap::new();
        assert!(parse_ir_params_resolved("", &m).is_empty());
        assert!(parse_ir_params_resolved("...", &m).is_empty());
    }

    #[test]
    fn fallback_name_when_unnamed() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("i32", 3, &m).unwrap();
        assert_eq!(p.name, "arg3");
    }

    #[test]
    fn numeric_name_is_arg_prefixed() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("i32 %5", 0, &m).unwrap();
        assert_eq!(p.name, "arg5");
    }

    #[test]
    fn align_keyword_doesnt_leak_into_type() {
        let m = HashMap::new();
        let p = parse_ir_param_resolved("ptr align 8 %p", 0, &m).unwrap();
        assert_eq!(p.name, "p");
        assert!(matches!(p.ty, TypeInfo::Pointer(_)));
    }
}
