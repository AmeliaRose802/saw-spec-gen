//! Function extraction from mir-json.

use super::adt::build_adt_map;
use super::node::{ArgDef, FnDef, MirJson};
use super::rust_types::{parse_mir_return_type, parse_rust_type_string};
use crate::constraints::{Annotation, FunctionInfo, ParamInfo, TypeInfo};
use anyhow::Result;
use std::collections::HashMap;

/// Extract every function declaration in `mir`, optionally filtered by
/// name substring.
pub fn extract_functions(mir: &MirJson, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    let adt_map = build_adt_map(mir);
    let mut functions = Vec::new();
    for fn_def in &mir.fns {
        let Some(func) = parse_mir_function(fn_def, &adt_map) else {
            continue;
        };
        if filter.map(|f| func.name.contains(f)).unwrap_or(true) {
            functions.push(func);
        }
    }
    Ok(functions)
}

/// Convert a single [`FnDef`] into a [`FunctionInfo`]. Returns `None`
/// when the name is missing.
pub fn parse_mir_function(
    func: &FnDef,
    adt_map: &HashMap<String, TypeInfo>,
) -> Option<FunctionInfo> {
    let name = func.name.clone()?;
    let is_async_fn = is_async_function(func);

    let params = func
        .args
        .iter()
        .filter_map(|a| parse_mir_param(a, adt_map))
        .collect();
    let return_type = func
        .return_ty
        .as_ref()
        .and_then(|v| parse_mir_return_type(v, adt_map))
        .unwrap_or(TypeInfo::Void);
    let return_type = if is_async_fn {
        TypeInfo::Opaque {
            name: format!("async_coroutine<{name}>"),
            size_bytes: 0,
        }
    } else {
        return_type
    };

    let annotations = collect_annotations(func);

    Some(FunctionInfo {
        name,
        mangled_name: None,
        params,
        return_type,
        // Rust has no C++-style exceptions; nounwind/noexcept annotations
        // appear above as `Annotation::NoThrow` for downstream display
        // purposes only.
        can_throw: false,
        is_virtual: false,
        has_body: true,
        is_system: false,
        annotations,
        referenced_globals: vec![],
        called_functions: vec![],
    })
}

/// Convert a single [`ArgDef`] into a [`ParamInfo`].
pub fn parse_mir_param(arg: &ArgDef, adt_map: &HashMap<String, TypeInfo>) -> Option<ParamInfo> {
    let name = arg.name.clone()?;
    let ty_name = arg.ty.as_deref()?;
    let is_mut = arg.mutability.as_ref().map(|m| m.is_mut()).unwrap_or(false);
    let (ty, mutability, nullable) = parse_rust_type_string(ty_name, is_mut, adt_map);
    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations: Vec::new(),
    })
}

/// `is_async` is set explicitly *or* the return-type string mentions a
/// known coroutine signature (`GenFuture`, `Coroutine`, `impl Future`,
/// `Poll`).
fn is_async_function(func: &FnDef) -> bool {
    if func.is_async == Some(true) {
        return true;
    }
    let Some(ret) = func.return_ty.as_ref().and_then(|v| v.as_str()) else {
        return false;
    };
    ret.contains("GenFuture")
        || ret.contains("Coroutine")
        || ret.contains("impl Future")
        || ret.contains("Poll")
}

/// Collect declared function-level attributes that map to a
/// [`crate::constraints::Annotation`].
fn collect_annotations(func: &FnDef) -> Vec<Annotation> {
    let mut out = Vec::new();
    for attr in &func.attrs {
        if let Some(name) = attr.as_str() {
            if matches!(name, "nounwind" | "noexcept") {
                out.push(Annotation::NoThrow);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn from_json(v: serde_json::Value) -> MirJson {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn extracts_simple_function() {
        let mir = from_json(json!({
            "fns": [{
                "name": "add",
                "args": [
                    {"name": "a", "ty": "ty::Int::I32", "mut": {"kind": "Not"}},
                    {"name": "b", "ty": "ty::Int::I32", "mut": {"kind": "Not"}}
                ],
                "return_ty": "ty::Int::I32"
            }]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
        assert!(!funcs[0].can_throw);
    }

    #[test]
    fn ref_params_pick_up_mutability() {
        let mir = from_json(json!({
            "fns": [{
                "name": "process",
                "args": [
                    {"name": "input", "ty": "ty::Ref", "mut": {"kind": "Not"}},
                    {"name": "output", "ty": "ty::Ref", "mut": {"kind": "Mut"}}
                ],
                "return_ty": "()"
            }]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert_eq!(
            funcs[0].params[0].mutability,
            crate::constraints::Mutability::Readonly
        );
        assert_eq!(
            funcs[0].params[0].nullable,
            crate::constraints::Nullability::NonNull,
        );
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
        assert_eq!(
            funcs[0].params[1].mutability,
            crate::constraints::Mutability::Mutable,
        );
    }

    #[test]
    fn async_function_return_becomes_coroutine_opaque() {
        let mir = from_json(json!({
            "fns": [{
                "name": "async_fetch",
                "is_async": true,
                "args": [],
                "return_ty": "impl Future"
            }]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        match &funcs[0].return_type {
            TypeInfo::Opaque { name, .. } => assert!(name.contains("async_coroutine")),
            other => panic!("expected Opaque coroutine, got {other:?}"),
        }
    }

    #[test]
    fn poll_return_type_implies_coroutine() {
        let mir = from_json(json!({
            "fns": [{"name": "poll_fn", "args": [], "return_ty": "Poll"}]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert!(matches!(funcs[0].return_type, TypeInfo::Opaque { .. }));
    }

    #[test]
    fn nounwind_attribute_emits_nothrow_annotation() {
        let mir = from_json(json!({
            "fns": [{
                "name": "safe",
                "args": [],
                "return_ty": "()",
                "attrs": ["nounwind"]
            }]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert!(funcs[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoThrow)));
    }

    #[test]
    fn filter_limits_by_name_substring() {
        let mir = from_json(json!({
            "fns": [
                {"name": "alpha", "args": [], "return_ty": "()"},
                {"name": "beta", "args": [], "return_ty": "()"}
            ]
        }));
        let funcs = extract_functions(&mir, Some("alp")).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "alpha");
    }

    #[test]
    fn unnamed_arg_is_dropped() {
        let mir = from_json(json!({
            "fns": [{
                "name": "f",
                "args": [{"ty": "u32", "mut": {"kind": "Not"}}],
                "return_ty": "()"
            }]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert!(funcs[0].params.is_empty());
    }

    #[test]
    fn fn_without_name_is_skipped() {
        let mir = from_json(json!({
            "fns": [{"args": [], "return_ty": "()"}]
        }));
        let funcs = extract_functions(&mir, None).unwrap();
        assert!(funcs.is_empty());
    }
}
