//! Merge LLVM IR-derived `dereferenceable(N)` annotations into the
//! [`AliasFallbacks`] map produced by [`crate::alias_fallbacks::collect_type_sizes`].
//!
//! The clang AST never carries LLVM-level attributes — only the IR
//! parser produces `Annotation::Dereferenceable(N)`.  This module
//! bridges the two by matching AST functions to IR functions via
//! mangled name, then copying deref sizes onto the AST-side type names
//! that the rewriter looks up.

use crate::alias_fallbacks::{deref_annotation, pointee_name, AliasFallbacks};
use crate::constraints::{Annotation, FunctionInfo, TypeInfo};
use std::collections::HashMap;

/// Merge LLVM IR-derived `dereferenceable(N)` annotations into `fb`.
///
/// Runs two passes:
///
///   1. **AST-matched merge** — for each AST function we look up the
///      matching IR function by mangled name and copy each IR
///      parameter's deref size onto the AST param's pointee type name.
///      Sret slot deref → AST return type's name.  This is the most
///      precise mapping (one AST type name per IR param).
///   2. **IR-only fallback** — for every IR parameter whose pointee
///      type is itself a known struct in the IR struct table, we
///      attribute the deref directly to the struct's short name (the
///      C++ class name, stripped of namespaces).  Catches:
///        - Virtual interface methods that the AST records under a
///          different mangled name than the IR uses for the same
///          symbol (rare but happens with MSVC vtable thunks).
///        - Functions where the AST's `parentDeclContextId` resolution
///          failed and the mangled name on the AST node doesn't match
///          the IR's `define`/`declare` line exactly.
///        - sret return slots of `std::tuple<…>` whose IR struct name
///          gets sufficient size info via the deref attribute even when
///          the AST function isn't found.
pub fn add_ir_deref_fallbacks(
    fb: &mut AliasFallbacks,
    ast_funcs: &[FunctionInfo],
    ir_funcs: &[FunctionInfo],
) {
    merge_via_ast_match(fb, ast_funcs, ir_funcs);
    merge_via_ir_only(fb, ir_funcs);
}

/// Pass 1: precise AST↔IR match by mangled name.
fn merge_via_ast_match(
    fb: &mut AliasFallbacks,
    ast_funcs: &[FunctionInfo],
    ir_funcs: &[FunctionInfo],
) {
    let mut ir_by_name: HashMap<&str, &FunctionInfo> = HashMap::new();
    for f in ir_funcs {
        ir_by_name.insert(f.name.as_str(), f);
    }
    for ast in ast_funcs {
        let Some(ir) = find_ir_match(ast, &ir_by_name, ir_funcs) else {
            continue;
        };

        let sret_offset = match ir.params.first() {
            Some(p) if is_sret_param(&p.annotations) => 1,
            _ => 0,
        };

        if sret_offset == 1 {
            if let (Some(sret), Some(name)) =
                (ir.params.first(), pointee_name(&ast.return_type))
            {
                if let Some(n) = deref_annotation(&sret.annotations) {
                    fb.insert_bytes(name, n);
                }
            }
        }

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

/// Locate an IR function that corresponds to `ast`.
///
///   1. Exact match on the AST's mangled name (the canonical path).
///   2. Failing that, if the AST function's name appears as a unique
///      *suffix* of exactly one IR mangled name, accept that match.
///      Catches the case where the AST node has no `mangledName`
///      (rare — happens for some virtual / abstract methods clang
///      dumps without mangling) but the IR symbol is unambiguous.
fn find_ir_match<'a>(
    ast: &FunctionInfo,
    ir_by_name: &HashMap<&str, &'a FunctionInfo>,
    ir_funcs: &'a [FunctionInfo],
) -> Option<&'a FunctionInfo> {
    if let Some(mangled) = ast.mangled_name.as_deref() {
        if let Some(ir) = ir_by_name.get(mangled).copied() {
            return Some(ir);
        }
    }
    // Fallback: unique unmangled-name match.  C++ method names appear
    // as substrings of their mangled form (e.g. `HandleRequest` →
    // `?HandleRequest@MyClass@@…`).  Only accept when exactly one IR
    // function contains the name to avoid attributing deref sizes to
    // the wrong overload.
    let needle = ast.name.as_str();
    if needle.is_empty() {
        return None;
    }
    let matches: Vec<&FunctionInfo> = ir_funcs
        .iter()
        .filter(|f| f.name.contains(needle))
        .collect();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

/// Pass 2: attribute IR deref sizes to the IR pointee's short name
/// directly, ignoring AST matching entirely.
///
/// LLVM IR carries the struct name on `sret(%struct.Foo::Bar::X)`
/// attributes and (when not lowered to opaque `ptr`) on the type-string
/// of struct-typed parameters.  We record the deref size under both
/// the *fully-qualified* C++ name (with namespaces, matching e.g.
/// `std::tuple<…>`) and the short last-component name (matching e.g.
/// `HttpRequest` after the rewriter's suffix-strip) — the rewriter
/// looks up whichever form `type_to_saw` happened to emit.
fn merge_via_ir_only(fb: &mut AliasFallbacks, ir_funcs: &[FunctionInfo]) {
    for ir in ir_funcs {
        for p in &ir.params {
            let Some(n) = deref_annotation(&p.annotations) else {
                continue;
            };
            for key in ir_pointee_name_variants(&p.ty) {
                fb.insert_bytes(key, n);
            }
        }
    }
}

/// Extract the candidate C++ name keys from an IR-side `TypeInfo`.
///
/// Returns up to two keys:
///   1. The fully-qualified C++ name (`std::tuple<…>` once
///      `struct.`/`class.` is stripped).
///   2. The short last-component name (`HttpRequest` after the final
///      `::`).  Only emitted when it differs from the fully-qualified
///      form and is non-empty.
fn ir_pointee_name_variants(ty: &TypeInfo) -> Vec<&str> {
    let target = match ty {
        TypeInfo::Pointer(inner) => inner.as_ref(),
        other => other,
    };
    let name = match target {
        TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => name.as_str(),
        _ => return Vec::new(),
    };
    let full = name
        .strip_prefix("struct.")
        .or_else(|| name.strip_prefix("class."))
        .or_else(|| name.strip_prefix("union."))
        .unwrap_or(name);
    if full.is_empty() {
        return Vec::new();
    }
    let short = full.rsplit("::").next().unwrap_or(full);
    if short.is_empty() || short == full {
        vec![full]
    } else {
        vec![full, short]
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
        // No AST match AND IR pointee is a raw i8 pointer (no struct
        // name) — neither pass should record anything for "X".
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

    #[test]
    fn ir_only_fallback_picks_up_qualified_struct_short_name() {
        // No AST function provided.  IR has a function whose param
        // pointee is a fully-qualified struct name with deref(112).
        // The IR-only pass must record BOTH the full name and the
        // short tail under the deref size.
        let ir = ir_func(
            "?unknown_mangling@@",
            vec![ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "struct.Microsoft::Azure::Host::HttpRequest".into(),
                    size_bytes: 0,
                })),
                Some(112),
                false,
            )],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[], &[ir]);
        assert_eq!(fb.bytes.get("HttpRequest").copied(), Some(112));
        assert_eq!(
            fb.bytes
                .get("Microsoft::Azure::Host::HttpRequest")
                .copied(),
            Some(112)
        );
    }

    #[test]
    fn ir_only_fallback_keeps_template_name_intact() {
        // `std::tuple<...>` should be recorded under its full
        // C++-namespaced form so the rewriter's exact-match on
        // `llvm_alias "std::tuple<...>"` finds it.  We also record the
        // short tail (`tuple<...>`) but it's irrelevant for the
        // rewriter's purposes — the test is mostly to catch
        // regressions in name parsing.
        let full = "struct.std::tuple<KeyStoreOperationResult, LatchableKey>";
        let ir = ir_func(
            "?some_func@@",
            vec![ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: full.into(),
                    size_bytes: 0,
                })),
                Some(48),
                true,
            )],
            TypeInfo::Void,
        );
        let mut fb = AliasFallbacks::default();
        add_ir_deref_fallbacks(&mut fb, &[], &[ir]);
        let expected = "std::tuple<KeyStoreOperationResult, LatchableKey>";
        assert_eq!(fb.bytes.get(expected).copied(), Some(48));
    }
}
