//! Derivation of [`SpecConstraint`] values from [`FunctionInfo`].

use super::length_companion::{guess_with_shared_flag, LengthCompanionGuess};
use super::saw_type::{pointee_saw_type, type_to_saw};
use super::types::{
    AllocType, Annotation, FunctionInfo, Mutability, Nullability, ParamConstraint,
    ReturnConstraint, SpecConstraint, TypeInfo,
};
use super::value_clauses::value_clauses;
use anyhow::Result;
use std::collections::HashMap;

/// Default symbolic upper bound used when a pointer parameter has a
/// parameter-reference SAL annotation (`_In_reads_(<name>)` /
/// `_Out_writes_(<name>)`) rather than a decimal literal length. The
/// emitted spec allocates `[MAX][8]` for the buffer and adds an
/// `llvm_precond {{ <name> <= MAX }}` to bound the dynamic length.
///
/// 16 is large enough to cover the common short-buffer cases
/// (UUIDs / handles / counters) without exploding solver time. Users
/// who need larger buffers can either:
///   - hand-edit the generated `verify.saw` to use a different bound
///     and matching precondition, or
///   - file the constant as a CLI knob (`--paramref-max-len`) in a
///     follow-up.
pub const DEFAULT_PARAMREF_MAX_LEN: usize = 16;

/// Derive SAW constraints from function info.
pub fn derive_constraints(functions: &[FunctionInfo]) -> Result<Vec<SpecConstraint>> {
    functions.iter().map(derive_function_constraints).collect()
}

fn derive_function_constraints(func: &FunctionInfo) -> Result<SpecConstraint> {
    let mut params = Vec::new();
    let mut postconditions = Vec::new();

    // Collect parameter names up-front so unsized-pointer warnings can
    // point the user at a likely length-companion parameter
    // (e.g. `buf` + `buf_len`, or `n` + any pointer).
    let all_param_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();

    // Pre-compute the inbound-pointer count for every parameter name:
    // how many *pointer* parameters in this function would name-match
    // it as a length companion? When the count is > 1 we have a copy-
    // pattern ambiguity (`void copy(void* src, void* dst, size_t n)`)
    // and the heuristic must refuse to commit to either pointer.
    let shared_companions: HashMap<String, usize> = build_shared_companion_map(&func.params);

    // Check function-level annotations for NoThrow override
    let can_throw = if func
        .annotations
        .iter()
        .any(|a| matches!(a, Annotation::NoThrow))
    {
        false
    } else {
        func.can_throw
    };

    for param in &func.params {
        let is_indirect = matches!(&param.ty, TypeInfo::Pointer(_));
        let inner_ty = match &param.ty {
            TypeInfo::Pointer(inner) => inner.as_ref(),
            other => other,
        };

        // For pointer parameters, an `_In_reads_(N)` / `_In_reads_bytes_(N)`
        // / `_Out_writes_(N)` annotation upgrades the allocation from a
        // single element to an `N`-element array.  Without this, a
        // `const char* s` annotated `_In_reads_(8)` would allocate only
        // one byte and every `s[i]` for i > 0 would fault with "outside
        // of the allocation".  The annotation is the only way the spec
        // generator can know the caller's buffer-size contract.
        //
        // Returns `(N, Option<paramref>)`: the second element is
        // `Some(name)` when the length came from a sibling-parameter
        // reference; that name is used downstream to emit an
        // `llvm_precond {{ name <= N }}` bound.
        let buffer_len: Option<(usize, Option<String>)> = if is_indirect {
            param.annotations.iter().find_map(|a| match a {
                Annotation::InReads(n) | Annotation::OutWrites(n) | Annotation::InZ(n)
                    if *n > 1 =>
                {
                    Some((*n, None))
                }
                Annotation::InReadsParam(p) | Annotation::OutWritesParam(p) => {
                    Some((DEFAULT_PARAMREF_MAX_LEN, Some(p.clone())))
                }
                _ => None,
            })
        } else {
            None
        };

        // Bug #15: a raw `void*` (without a buffer-size annotation) is
        // almost always used as a pointer-value, not as a backing
        // buffer — e.g. `bool isNonNull(void* p)` reads the address,
        // never `*p`. Allocating a 1-byte buffer for it would (a) force
        // the pointer to be non-null, blocking the very case the
        // function checks, and (b) hand the *buffer contents* to the
        // Cryptol spec instead of the address. Lower these as a fresh
        // 64-bit symbolic value so `llvm_execute_func [llvm_term p]`
        // passes the address and the Cryptol spec sees the address.
        // Opt back into buffer modeling by attaching an `_In_reads_(N)`
        // (or sibling) annotation.
        let void_ptr_as_value =
            is_indirect && matches!(inner_ty, TypeInfo::Void) && buffer_len.is_none();

        let element_saw = if is_indirect {
            pointee_saw_type(inner_ty)
        } else {
            type_to_saw(inner_ty)
        };
        let saw_type = if void_ptr_as_value {
            // Pointer-sized integer: passes through `llvm_term p` to the
            // function and to the Cryptol callsite as the same `[64]`.
            "llvm_int 64".to_string()
        } else {
            match &buffer_len {
                Some((n, _)) => format!("llvm_array {n} ({element_saw})"),
                None => element_saw,
            }
        };

        let (alloc_type, unchanged_after) = if void_ptr_as_value {
            // Treat the address as a scalar passed by value — no
            // backing buffer, no `unchanged_after` round-tripping.
            (AllocType::FreshVar, false)
        } else if is_indirect {
            match param.mutability {
                Mutability::Readonly => (AllocType::AllocReadonly, true),
                Mutability::Mutable => (AllocType::AllocMutable, false),
                Mutability::WriteOnly => (AllocType::AllocMutable, false),
            }
        } else {
            match inner_ty {
                // Scalar / scalar-like types pass by value through an
                // `iN` register. Enums must be in this list too — the
                // discriminant is a plain integer of `discriminant_bits`
                // width and treating it as `AllocReadonly` makes SAW
                // reject the call with "declared iN, provided iN*".
                TypeInfo::Bool
                | TypeInfo::SignedInt(_)
                | TypeInfo::UnsignedInt(_)
                | TypeInfo::Float(_)
                | TypeInfo::Enum { .. } => (AllocType::FreshVar, false),
                _ => match param.mutability {
                    Mutability::Readonly => (AllocType::AllocReadonly, true),
                    Mutability::Mutable | Mutability::WriteOnly => (AllocType::AllocMutable, false),
                },
            }
        };

        let mut preconditions = Vec::new();

        // Loud-TODO emission for unsized pointers. Previously this was
        // a "may be null -- spec assumes non-null" note, which was
        // misleading: the underlying allocation is a single element,
        // and the resulting spec is almost certainly wrong for any
        // length-prefixed buffer parameter. Replace with an explicit
        // TODO that names a likely length-companion sibling parameter,
        // so the user can either add a SAL annotation upstream or
        // hand-edit the generated allocation.
        let has_size_annotation = param.annotations.iter().any(|a| {
            matches!(
                a,
                Annotation::InReads(_)
                    | Annotation::OutWrites(_)
                    | Annotation::InReadsParam(_)
                    | Annotation::OutWritesParam(_)
                    | Annotation::InZ(_)
                    | Annotation::Dereferenceable(_)
            )
        });
        if is_indirect
            && !void_ptr_as_value
            && !matches!(alloc_type, AllocType::FreshVar)
            && !has_size_annotation
        {
            let guess: LengthCompanionGuess = guess_with_shared_flag(
                &param.name,
                &all_param_names,
                any_shared_companion(&param.name, &all_param_names, &shared_companions),
            );
            preconditions.extend(guess.todo_lines());
        } else if param.nullable == Nullability::Nullable
            && !matches!(alloc_type, AllocType::FreshVar)
        {
            preconditions.push(format!(
                "// NOTE: {} may be null -- spec assumes non-null",
                param.name,
            ));
        }

        // Type-driven value constraints (saw-spec-gen-xip). The
        // dispatcher lives in [`super::value_clauses`]; we only emit
        // for FreshVar params because indirect (pointer-passed) values
        // are named `{name}_before` in the auto-spec but `{name}` in
        // the equiv-spec body. Indirect users currently have no
        // auto-precondition surface for value clauses; reach for the
        // ArrayView annotation surfaces (Cryptol `[n][T]`, SAL
        // `_In_reads_` / `SAW_BUF`, or a sidecar `.spec.toml`) to
        // bind a length when needed.
        if matches!(alloc_type, AllocType::FreshVar) {
            for clause in value_clauses(inner_ty, &param.name) {
                preconditions.push(format!("llvm_precond {{{{ {clause} }}}}"));
            }
        }

        // Annotation-driven constraints
        for ann in &param.annotations {
            match ann {
                Annotation::InReads(n) if *n > 0 => {
                    preconditions.push(format!(
                        "// _In_reads_({n}) -- readable buffer of {n} elements"
                    ));
                }
                Annotation::InZ(n) if *n > 0 => {
                    preconditions.push(format!(
                        "// _In_z_({n}) -- NUL-terminated string, up to {n} bytes"
                    ));
                    // `_In_z_(N)` means there exists a NUL byte inside the
                    // first `N` elements. The parameter variable name is in
                    // scope at both callsites (`verify.saw` equiv specs and
                    // per-function auto specs), so we can emit this directly
                    // with no additional helper imports.
                    preconditions.push(format!(
                        "llvm_precond {{{{ any (\\b -> b == 0) {} }}}}",
                        param.name
                    ));
                }
                Annotation::OutWrites(n) if *n > 0 => {
                    preconditions.push(format!(
                        "// _Out_writes_({n}) -- writable buffer for {n} elements"
                    ));
                }
                Annotation::InReadsParam(pname) | Annotation::OutWritesParam(pname) => {
                    let kind = if matches!(ann, Annotation::InReadsParam(_)) {
                        "_In_reads_"
                    } else {
                        "_Out_writes_"
                    };
                    preconditions.push(format!(
                        "// TODO[saw-spec-gen]: {kind}({pname}) -- buffer length conveyed by",
                    ));
                    preconditions.push(format!(
                        "//   sibling parameter `{pname}`. Auto-spec sized buffer to upper bound",
                    ));
                    preconditions.push(format!(
                        "//   {DEFAULT_PARAMREF_MAX_LEN}; raise/lower by hand if needed.",
                    ));
                    preconditions.push(format!(
                        "llvm_precond {{{{ ({pname} : [64]) <= {DEFAULT_PARAMREF_MAX_LEN} }}}}",
                    ));
                }
                Annotation::Dereferenceable(n) => {
                    preconditions.push(format!("// dereferenceable({n}) bytes"));
                }
                Annotation::NoAlias => {
                    preconditions.push("// noalias -- does not alias other params".into());
                }
                Annotation::NoCapture => {
                    preconditions.push("// nocapture -- pointer not retained after call".into());
                }
                _ => {}
            }
        }

        if unchanged_after {
            postconditions.push(format!(
                "llvm_points_to {name}_ptr (llvm_term {name}_before)",
                name = param.name,
            ));
        }

        params.push(ParamConstraint {
            name: param.name.clone(),
            alloc_type,
            saw_type,
            preconditions,
            unchanged_after,
            dereferenceable_size: param.annotations.iter().find_map(|a| match a {
                Annotation::Dereferenceable(n) => Some(*n),
                _ => None,
            }),
            out_postcond: None,
        });
    }

    let return_constraint = derive_return_constraint(&func.return_type);

    // Function-level annotation handling
    for ann in &func.annotations {
        match ann {
            Annotation::MustInspectResult => {
                postconditions
                    .push("// _Must_inspect_result_ -- return value must be checked".into());
            }
            Annotation::Custom(s) => {
                postconditions.push(format!("// annotation: {s}"));
            }
            _ => {}
        }
    }

    Ok(SpecConstraint {
        function_name: func.name.clone(),
        mangled_name: func.mangled_name.clone(),
        params,
        return_constraint,
        can_throw,
        is_virtual: func.is_virtual,
        has_body: func.has_body,
        postconditions,
        referenced_globals: func.referenced_globals.clone(),
    })
}

fn derive_return_constraint(ty: &TypeInfo) -> ReturnConstraint {
    let mut value_constraints = Vec::new();

    // The return value is always named `ret` in the emitter (see
    // `llvm_return.rs` / `mir_spec.rs`), so we feed that name to the
    // generic [`value_clauses`] dispatcher and wrap each clause as a
    // postcondition. New constrained return types automatically pick
    // this up.
    for clause in value_clauses(ty, "ret") {
        value_constraints.push(format!("llvm_postcond {{{{ {clause} }}}}"));
    }

    if let TypeInfo::UnsignedInt(bits) = ty {
        value_constraints.push(format!("// unsigned {bits}-bit -- no extra constraint"));
    }

    // sret detection: any aggregate that the MSVC x64 ABI lowers to a
    // hidden output-pointer parameter.
    //
    // - `TypeInfo::Struct` always passes via sret in MSVC: the AST may
    //   have failed to compute `size_bytes` (e.g. the struct contains
    //   `std::optional<T>` whose size we don't model), but the ABI
    //   classification is independent of our size resolution.
    // - `TypeInfo::Opaque` with a known size > 8 bytes is an
    //   unresolved aggregate; treat as sret.
    // - `TypeInfo::Opaque` with size 0 whose name looks like a
    //   templated/qualified C++ type (`std::tuple<…>`, `Foo::Bar`) is
    //   almost certainly an aggregate the AST couldn't expand; treat
    //   as sret here so [`crate::emit::saw_emit::verify_script_steps`]
    //   inserts a `result_ptr` arg rather than silently dropping it.
    let is_sret = match ty {
        TypeInfo::Struct { .. } => true,
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8 => true,
        TypeInfo::Opaque {
            size_bytes: 0,
            name,
        } if name.contains('<') || name.contains("::") => true,
        _ => false,
    };

    let returns_pointer = matches!(ty, TypeInfo::Pointer(_));

    // For pointer returns, the SAW type should be the pointee type (for llvm_alloc)
    let saw_type = if returns_pointer {
        match ty {
            TypeInfo::Pointer(inner) => pointee_saw_type(inner),
            _ => type_to_saw(ty),
        }
    } else {
        type_to_saw(ty)
    };

    ReturnConstraint {
        saw_type,
        value_constraints,
        is_sret,
        returns_pointer,
        sret_prestate: false,
    }
}

/// Override the AST-derived sret classification using the LLVM IR
/// signature.  On MSVC x64, small trivially-copyable aggregates
/// (≤ 8 bytes) are returned in a register — the IR has a scalar
/// return type (`iN`) and no `sret` parameter.  The AST heuristic
/// (`TypeInfo::Struct → sret`) can't see the ABI lowering, so this
/// function corrects it when IR data is available.
pub fn correct_sret_from_ir(spec: &mut SpecConstraint, ir_funcs: &[FunctionInfo]) {
    if !spec.return_constraint.is_sret || ir_funcs.is_empty() {
        return;
    }
    let Some(mangled) = &spec.mangled_name else {
        return;
    };
    // LLVM IR quotes names with special characters (e.g.
    // `@"?foo@@YAUHX@@XZ"`); strip the surrounding quotes.
    let ir_fn = ir_funcs
        .iter()
        .find(|f| f.name.trim_matches('"') == mangled);
    let Some(ir_fn) = ir_fn else { return };

    // After `extract_sret` in the IR parser, a genuine sret function
    // has its return type promoted to the inner struct.  A register-
    // return function keeps the original scalar type.  If the IR
    // return is a scalar (not void / struct / large opaque), the
    // aggregate fits in a register.
    let is_register = matches!(
        ir_fn.return_type,
        TypeInfo::Bool | TypeInfo::SignedInt(_) | TypeInfo::UnsignedInt(_) | TypeInfo::Enum { .. }
    );
    if is_register {
        spec.return_constraint.is_sret = false;
        spec.return_constraint.saw_type = type_to_saw(&ir_fn.return_type);
    }
}

/// For every parameter, count how many *pointer* parameters in this
/// function would name-match it as a length companion via the
/// [`super::length_companion`] heuristic. The map's keys are
/// parameter names; values are inbound-pointer counts.
///
/// Used to detect copy-pattern ambiguity: in
/// `void copy(void* src, void* dst, size_t n)`, `n` matches both
/// `src` and `dst`. Either guess in isolation looks unambiguous, so
/// the per-pointer heuristic needs this map to know it must refuse.
fn build_shared_companion_map(params: &[super::types::ParamInfo]) -> HashMap<String, usize> {
    use super::length_companion::guess;
    let all_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in params {
        if !matches!(&p.ty, TypeInfo::Pointer(_)) {
            continue;
        }
        // We don't want to count `void*` parameters that the rest of
        // derive() will pass-by-value (no buffer behind them); but the
        // ambiguity test is cheap and the false-positive cost (forcing
        // AMBIGUOUS in a corner case) is small, so include all pointers.
        let g = guess(&p.name, &all_names);
        for cand in g.candidate_names_for_internal_use() {
            *counts.entry(cand).or_insert(0) += 1;
        }
    }
    counts
}

/// True iff *any* candidate the heuristic would propose for
/// `ptr_name` is currently claimed by another pointer parameter too.
fn any_shared_companion(
    ptr_name: &str,
    all_names: &[String],
    shared: &HashMap<String, usize>,
) -> bool {
    use super::length_companion::guess;
    let g = guess(ptr_name, all_names);
    g.candidate_names_for_internal_use()
        .iter()
        .any(|c| shared.get(c).copied().unwrap_or(0) > 1)
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "derive_annotation_tests.rs"]
mod annotation_tests;
