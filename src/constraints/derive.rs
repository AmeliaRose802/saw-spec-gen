//! Derivation of [`SpecConstraint`] values from [`FunctionInfo`].

use super::saw_type::type_to_saw;
use super::types::{
    AllocType, Annotation, FunctionInfo, Mutability, Nullability, ParamConstraint,
    ReturnConstraint, SpecConstraint, TypeInfo,
};
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
        let element_saw = type_to_saw(inner_ty);
        let saw_type = match buffer_len {
            Some(n) => format!("llvm_array {n} ({element_saw})"),
            None => element_saw,
        };

        let (alloc_type, unchanged_after) = if is_indirect {
            match param.mutability {
                Mutability::Readonly => (AllocType::AllocReadonly, true),
                Mutability::Mutable => (AllocType::AllocMutable, false),
                Mutability::WriteOnly => (AllocType::AllocMutable, false),
            }
        } else {
            match inner_ty {
                TypeInfo::Bool | TypeInfo::SignedInt(_) | TypeInfo::UnsignedInt(_) => {
                    (AllocType::FreshVar, false)
                }
                _ => match param.mutability {
                    Mutability::Readonly => (AllocType::AllocReadonly, true),
                    Mutability::Mutable => (AllocType::AllocMutable, false),
                    Mutability::WriteOnly => (AllocType::AllocMutable, false),
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

        // Type-level value constraints
        match inner_ty {
            TypeInfo::Bool => {}
            TypeInfo::Enum {
                variants,
                discriminant_bits,
                ..
            } => {
                let max_disc = variants.len() as u64 - 1;
                preconditions.push(format!(
                    "llvm_precond {{{{ {name}_disc <= ({max} : [{bits}]) }}}}",
                    name = param.name,
                    max = max_disc,
                    bits = discriminant_bits,
                ));
            }
            TypeInfo::Option(_) => {
                preconditions.push(format!(
                    "llvm_precond {{{{ {name}_disc <= (1 : [8]) }}}}",
                    name = param.name,
                ));
            }
            _ => {}
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

    match ty {
        TypeInfo::Bool => {}
        TypeInfo::Enum {
            variants,
            discriminant_bits,
            ..
        } => {
            let max_disc = variants.len() as u64 - 1;
            value_constraints.push(format!(
                "llvm_postcond {{{{ ret_disc <= ({max} : [{bits}]) }}}}",
                max = max_disc,
                bits = discriminant_bits,
            ));
        }
        TypeInfo::Option(_) => {
            value_constraints.push("llvm_postcond {{ ret_disc <= (1 : [8]) }}".into());
        }
        TypeInfo::UnsignedInt(bits) => {
            value_constraints.push(format!("// unsigned {bits}-bit -- no extra constraint"));
        }
        _ => {}
    }

    let is_sret = matches!(ty, TypeInfo::Struct { .. })
        || matches!(ty, TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8);

    let returns_pointer = matches!(ty, TypeInfo::Pointer(_));

    // For pointer returns, the SAW type should be the pointee type (for llvm_alloc)
    let saw_type = if returns_pointer {
        match ty {
            TypeInfo::Pointer(inner) => {
                let inner_saw = type_to_saw(inner);
                if inner_saw == "// void" {
                    // void* → allocate a byte array
                    "llvm_array 64 (llvm_int 8)".to_string()
                } else {
                    inner_saw
                }
            }
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
mod tests {
    use super::*;
    use crate::constraints::ParamInfo;

    #[test]
    fn test_derive_readonly_param() {
        let func = FunctionInfo {
            name: "test_fn".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "x".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].params[0].alloc_type, AllocType::AllocReadonly);
        assert!(specs[0].params[0].unchanged_after);
    }

    #[test]
    fn test_derive_mutable_param() {
        let func = FunctionInfo {
            name: "test_fn".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "buf".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
                mutability: Mutability::Mutable,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert_eq!(specs[0].params[0].alloc_type, AllocType::AllocMutable);
        assert!(!specs[0].params[0].unchanged_after);
    }

    #[test]
    fn test_derive_freshvar_for_scalar() {
        let func = FunctionInfo {
            name: "add".into(),
            mangled_name: None,
            params: vec![
                ParamInfo {
                    name: "a".into(),
                    ty: TypeInfo::SignedInt(32),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                },
                ParamInfo {
                    name: "b".into(),
                    ty: TypeInfo::SignedInt(32),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                },
            ],
            return_type: TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert_eq!(specs[0].params[0].alloc_type, AllocType::FreshVar);
        assert_eq!(specs[0].params[1].alloc_type, AllocType::FreshVar);
    }

    #[test]
    fn test_derive_nullable_param_comment() {
        let func = FunctionInfo {
            name: "maybe_null".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "ptr".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
                mutability: Mutability::Readonly,
                nullable: Nullability::Nullable,
                annotations: vec![],
            }],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0].params[0]
            .preconditions
            .iter()
            .any(|p| p.contains("may be null")));
    }

    #[test]
    fn test_derive_enum_discriminant_constraint() {
        let func = FunctionInfo {
            name: "check_status".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "status".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::Enum {
                    name: "Status".into(),
                    variants: vec!["Ok".into(), "Err".into(), "Pending".into()],
                    discriminant_bits: 64,
                })),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Bool,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0].params[0]
            .preconditions
            .iter()
            .any(|p| p.contains("2")));
    }

    #[test]
    fn test_derive_nothrow_annotation() {
        let func = FunctionInfo {
            name: "safe_fn".into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: true,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![Annotation::NoThrow],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(!specs[0].can_throw);
    }

    #[test]
    fn test_derive_return_enum_constraint() {
        let func = FunctionInfo {
            name: "get_status".into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::Enum {
                name: "Status".into(),
                variants: vec!["A".into(), "B".into()],
                discriminant_bits: 32,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0]
            .return_constraint
            .value_constraints
            .iter()
            .any(|c| c.contains("ret_disc")));
    }

    #[test]
    fn test_derive_postconditions_readonly() {
        let func = FunctionInfo {
            name: "read_only".into(),
            mangled_name: None,
            params: vec![ParamInfo {
                name: "data".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(64))),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0]
            .postconditions
            .iter()
            .any(|p| p.contains("data_before")));
    }

    #[test]
    fn test_derive_must_inspect_result() {
        let func = FunctionInfo {
            name: "important".into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            called_functions: vec![],
            referenced_globals: vec![],
            annotations: vec![Annotation::MustInspectResult],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0]
            .postconditions
            .iter()
            .any(|p| p.contains("Must_inspect_result")));
    }
}
