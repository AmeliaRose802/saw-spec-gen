//! Merge LLVM IR-derived `dereferenceable(N)` annotations into the
//! [`AliasFallbacks`] map produced by [`crate::alias_fallbacks::collect_type_sizes`].
//!
//! The clang AST never carries LLVM-level attributes — only the IR
//! parser produces `Annotation::Dereferenceable(N)`.  This module
//! bridges the two by matching AST functions to IR functions via
//! mangled name, then copying deref sizes onto the AST-side type names
//! that the rewriter looks up.

use crate::alias_fallbacks::{deref_annotation, pointee_name, AliasFallbacks};
use crate::constraints::{Annotation, FunctionInfo};
use std::collections::HashMap;

/// Merge LLVM IR-derived `dereferenceable(N)` annotations into `fb`.
///
/// For each AST function we look up the matching IR function by mangled
/// name, then:
///
///   1. If the IR has an `sret` hidden parameter at position 0,
///      attribute its deref size to the AST's return type's name.
///      This is the only reliable source of size for `std::tuple<…>`
///      returned by value — the ABI rewrites the C++ source's
///      `tuple foo()` into
///      `void foo(sret(%struct.…tuple…) dereferenceable(N) ptr %ret, …)`.
///   2. For each remaining AST parameter, copy the IR parameter's
///      deref size (at the same position, offset by the sret slot if
///      present) onto the AST param's pointee type name.  Covers
///      forward-declared types like `HttpRequest` (size known from the
///      IR's `dereferenceable(112)` attribute but not from the AST's
///      empty field layout).
pub fn add_ir_deref_fallbacks(
    fb: &mut AliasFallbacks,
    ast_funcs: &[FunctionInfo],
    ir_funcs: &[FunctionInfo],
) {
    let mut ir_by_name: HashMap<&str, &FunctionInfo> = HashMap::new();
    for f in ir_funcs {
        ir_by_name.insert(f.name.as_str(), f);
    }
    for ast in ast_funcs {
        // Match by mangled name; the IR's function name *is* the
        // mangled symbol.  Functions whose AST entry lacks a mangled
        // name (e.g. `extern "C"` decls) won't merge — but they also
        // wouldn't appear under an `llvm_alias` reference because
        // there's no C++ class involved.
        let Some(mangled) = ast.mangled_name.as_deref() else {
            continue;
        };
        let Some(ir) = ir_by_name.get(mangled).copied() else {
            continue;
        };

        // Detect IR sret offset.  The IR parser tags the hidden return
        // pointer with `Annotation::Custom("sret")` on the first param.
        let sret_offset = match ir.params.first() {
            Some(p) if is_sret_param(&p.annotations) => 1,
            _ => 0,
        };

        // Sret deref size → AST return type's name.
        if sret_offset == 1 {
            if let (Some(sret), Some(name)) =
                (ir.params.first(), pointee_name(&ast.return_type))
            {
                if let Some(n) = deref_annotation(&sret.annotations) {
                    fb.insert_bytes(name, n);
                }
            }
        }

        // Per-param deref size → AST param's pointee name.
        for (i, ast_p) in ast.params.iter().enumerate() {
            let Some(ir_p) = ir.params.get(i + sret_offset) else {
                continue;
            };
            let Some(n) = deref_annotation(&ir_p.annotations) else {
                continue;
            };
            if let Some(name) = pointee_name(&ast_p.ty) {
                fb.insert_bytes(name, n);
            }
        }
    }
}

/// True when the annotation list contains the IR parser's `sret` marker.
fn is_sret_param(anns: &[Annotation]) -> bool {
    anns.iter()
        .any(|a| matches!(a, Annotation::Custom(s) if s == "sret"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{Mutability, Nullability, ParamInfo, TypeInfo};

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

    fn ast_func(mangled: &str, params: Vec<ParamInfo>, ret: TypeInfo) -> FunctionInfo {
        let mut f = empty_func();
        f.mangled_name = Some(mangled.into());
        f.params = params;
        f.return_type = ret;
        f
    }

    /// LLVM IR functions are keyed by mangled symbol — `parse_ir_function`
    /// puts the mangled name into `FunctionInfo.name`.
    fn ir_func(mangled: &str, params: Vec<ParamInfo>, ret: TypeInfo) -> FunctionInfo {
        let mut f = empty_func();
        f.name = mangled.into();
        f.params = params;
        f.return_type = ret;
        f
    }

    fn ir_param(ty: TypeInfo, deref: Option<usize>, sret: bool) -> ParamInfo {
        let mut anns = Vec::new();
        if let Some(n) = deref {
            anns.push(Annotation::Dereferenceable(n));
        }
        if sret {
            anns.push(Annotation::Custom("sret".into()));
        }
        ParamInfo {
            name: String::new(),
            ty,
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: anns,
        }
    }

    #[test]
    fn ir_deref_merges_param_size_onto_ast_pointee_name() {
        // AST has `void foo(HttpRequest*)` (forward-declared, no size).
        // IR has `void @foo(ptr dereferenceable(112) %req)`.  The merge
        // must record `HttpRequest → 112` so the rewriter can replace
        // `llvm_alias "HttpRequest"` with `llvm_array 112 (llvm_int 8)`.
        let ast = ast_func(
            "?foo@@YAXPEAVHttpRequest@@@Z",
            vec![param(
                "req",
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "HttpRequest".into(),
                    size_bytes: 0,
                })),
            )],
            TypeInfo::Void,
        );
        let ir = ir_func(
            "?foo@@YAXPEAVHttpRequest@@@Z",
            vec![ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                Some(112),
                false,
            )],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
        assert_eq!(fb.bytes.get("HttpRequest").copied(), Some(112));
    }

    #[test]
    fn ir_deref_merges_sret_size_onto_ast_return_type_name() {
        // AST has `std::tuple<…> foo()`.  IR rewrites to
        // `void @foo(sret(%struct.std::tuple<…>) dereferenceable(48) ptr %ret)`.
        // The merge must record `std::tuple<…> → 48` via the sret slot.
        let tuple_name =
            "std::tuple<KeyStoreOperationResult, LatchableKey>".to_string();
        let ast = ast_func(
            "?foo@@YA?AU?$tuple@UKeyStoreOperationResult@@ULatchableKey@@@std@@XZ",
            vec![],
            TypeInfo::Struct {
                name: tuple_name.clone(),
                size_bytes: None,
                fields: vec![],
            },
        );
        let ir = ir_func(
            "?foo@@YA?AU?$tuple@UKeyStoreOperationResult@@ULatchableKey@@@std@@XZ",
            vec![ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                Some(48),
                true,
            )],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
        assert_eq!(fb.bytes.get(&tuple_name).copied(), Some(48));
    }

    #[test]
    fn ir_deref_aligns_params_past_sret_slot() {
        // sret IR has a hidden first param; subsequent params must
        // align with AST params at the *same* index offset by one.
        let ast = ast_func(
            "?bar@@YA...",
            vec![param(
                "req",
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "HttpRequest".into(),
                    size_bytes: 0,
                })),
            )],
            TypeInfo::Struct {
                name: "Reply".into(),
                size_bytes: None,
                fields: vec![],
            },
        );
        let ir = ir_func(
            "?bar@@YA...",
            vec![
                // sret slot for the return Reply
                ir_param(
                    TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                    Some(32),
                    true,
                ),
                // real request param
                ir_param(
                    TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                    Some(112),
                    false,
                ),
            ],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
        assert_eq!(fb.bytes.get("Reply").copied(), Some(32));
        assert_eq!(fb.bytes.get("HttpRequest").copied(), Some(112));
    }

    #[test]
    fn ir_deref_skips_when_no_matching_mangled_name() {
        let ast = ast_func(
            "?a@@",
            vec![param(
                "p",
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "X".into(),
                    size_bytes: 0,
                })),
            )],
            TypeInfo::Void,
        );
        let ir = ir_func(
            "?b@@",
            vec![ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                Some(99),
                false,
            )],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
        assert!(fb.bytes.get("X").is_none());
    }
}
