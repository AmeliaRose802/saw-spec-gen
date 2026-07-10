//! Tests for [`super::alias_fallbacks_ir`].

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
    // Mirrors extract_sret (new behavior): sret sets WriteOnly AND keeps the annotation.
    if sret {
        anns.push(Annotation::Custom("sret".into()));
    }
    let mutability = if sret {
        Mutability::WriteOnly
    } else {
        Mutability::Mutable
    };
    ParamInfo {
        name: String::new(),
        ty,
        mutability,
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
    let tuple_name = "std::tuple<KeyStoreOperationResult, LatchableKey>".to_string();
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
    assert!(!fb.bytes.contains_key("X"));
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
        fb.bytes.get("Microsoft::Azure::Host::HttpRequest").copied(),
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

#[test]
fn sret_without_dereferenceable_uses_struct_type_size_ast_match() {
    // When the sret IR param carries no `dereferenceable(N)`, the AST-matched
    // merge must fall back to the byte size in the sret TypeInfo (Fix 1a).
    let ast = ast_func(
        "_ZN8KeyStore7currentEv",
        vec![],
        TypeInfo::Struct {
            name: "std::optional<EnrollmentKey>".into(),
            size_bytes: None,
            fields: vec![],
        },
    );
    let ir = ir_func(
        "_ZN8KeyStore7currentEv",
        vec![ir_param(
            TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: "class.std::optional<protocol::EnrollmentKey>".into(),
                size_bytes: Some(16),
                fields: vec![],
            })),
            None, // no dereferenceable annotation
            true,
        )],
        TypeInfo::Void,
    );
    let mut fb = AliasFallbacks::default();
    add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
    assert_eq!(
        fb.bytes.get("std::optional<EnrollmentKey>").copied(),
        Some(16)
    );
}

#[test]
fn sret_without_dereferenceable_uses_struct_type_size_ir_only() {
    // No AST match: IR-only pass must use TypeInfo size for sret without
    // dereferenceable annotation (Fix 1b).
    let ir = ir_func(
        "_ZN8KeyStore7currentEv",
        vec![ir_param(
            TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: "class.std::optional<protocol::EnrollmentKey>".into(),
                size_bytes: Some(16),
                fields: vec![],
            })),
            None, // no dereferenceable annotation
            true,
        )],
        TypeInfo::Void,
    );
    let mut fb = AliasFallbacks::default();
    add_ir_deref_fallbacks(&mut fb, &[], &[ir]);
    assert_eq!(
        fb.bytes
            .get("std::optional<protocol::EnrollmentKey>")
            .copied(),
        Some(16)
    );
}

#[test]
fn writeonly_non_sret_first_param_does_not_shift_alignment() {
    // Regression for false-positive sret detection: a plain `writeonly`
    // LLVM attribute (no sret annotation) must not be treated as sret.
    //
    // C++: `void write_value(Output* out, const Request* req)`
    // IR:  `void @write_value(ptr writeonly %out, ptr deref(32) %req)`
    //
    // %out has WriteOnly mutability but NO sret annotation — it is a
    // normal output pointer, not a hidden sret return slot.
    // sret_offset must stay 0: AST param 0 ("out") → IR param 0,
    //                          AST param 1 ("req") → IR param 1 (deref(32)).
    //
    // With the old WriteOnly-only check, sret_offset would be 1,
    // causing AST[0]="out" → IR[1]=deref(32) → `Output → 32` (WRONG)
    // and AST[1]="req" → IR[2]=missing → `Request` unrecorded (WRONG).
    let ast = ast_func(
        "?write_value@@",
        vec![
            param(
                "out",
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "Output".into(),
                    size_bytes: 0,
                })),
            ),
            param(
                "req",
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "Request".into(),
                    size_bytes: 0,
                })),
            ),
        ],
        TypeInfo::Void,
    );
    // Non-sret writeonly first param: WriteOnly mutability, no sret annotation.
    let ir_writeonly_first = ParamInfo {
        name: "out".into(),
        ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
        mutability: Mutability::WriteOnly,
        nullable: Nullability::NonNull,
        annotations: vec![], // no Annotation::Custom("sret")
    };
    let ir = ir_func(
        "?write_value@@",
        vec![
            ir_writeonly_first,
            ir_param(
                TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(8))),
                Some(32),
                false,
            ),
        ],
        TypeInfo::Void,
    );
    let mut fb = AliasFallbacks::default();
    add_ir_deref_fallbacks(&mut fb, &[ast], &[ir]);
    // Correct: deref(32) from IR[1] → AST[1] → records `Request → 32`.
    assert_eq!(fb.bytes.get("Request").copied(), Some(32));
    // Correct: `Output` must NOT be erroneously attributed deref(32).
    assert!(
        !fb.bytes.contains_key("Output"),
        "writeonly non-sret first param must not receive deref size"
    );
}
