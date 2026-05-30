//! Derivation of [`SpecConstraint`] values from [`FunctionInfo`].

use super::saw_type::{pointee_saw_type, type_to_saw};
use super::types::{
    AllocType, Annotation, FunctionInfo, Mutability, Nullability, ParamConstraint,
    ReturnConstraint, SpecConstraint, TypeInfo,
};
use super::value_clauses::value_clauses;
use anyhow::Result;

/// Derive SAW constraints from function info.
pub fn derive_constraints(functions: &[FunctionInfo]) -> Result<Vec<SpecConstraint>> {
    functions.iter().map(derive_function_constraints).collect()
}

fn derive_function_constraints(func: &FunctionInfo) -> Result<SpecConstraint> {
    let mut params = Vec::new();
    let mut postconditions = Vec::new();

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
        let buffer_len = if is_indirect {
            param.annotations.iter().find_map(|a| match a {
                Annotation::InReads(n) | Annotation::OutWrites(n) if *n > 1 => Some(*n),
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
            match buffer_len {
                Some(n) => format!("llvm_array {n} ({element_saw})"),
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
                | TypeInfo::Enum { .. } => (AllocType::FreshVar, false),
                _ => match param.mutability {
                    Mutability::Readonly => (AllocType::AllocReadonly, true),
                    Mutability::Mutable | Mutability::WriteOnly => (AllocType::AllocMutable, false),
                },
            }
        };

        let mut preconditions = Vec::new();

        // Nullable handling
        if param.nullable == Nullability::Nullable && !matches!(alloc_type, AllocType::FreshVar) {
            preconditions.push(format!(
                "// NOTE: {} may be null -- spec assumes non-null",
                param.name,
            ));
        }

        // Type-driven value constraints (saw-spec-gen-xip). The
        // dispatcher lives in [`super::value_clauses`]; we only emit
        // for FreshVar params because indirect (pointer-passed) values
        // are named `{name}_before` in the auto-spec but `{name}` in
        // the equiv-spec body. Indirect users can attach a precondition
        // via `--precond` until that naming is unified.
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
                Annotation::OutWrites(n) if *n > 0 => {
                    preconditions.push(format!(
                        "// _Out_writes_({n}) -- writable buffer for {n} elements"
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

    let is_sret = matches!(ty, TypeInfo::Struct { .. })
        || matches!(ty, TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8);

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
    }
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;
