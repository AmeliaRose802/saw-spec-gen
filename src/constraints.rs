//! Core constraint derivation logic.
//!
//! Takes a language-independent representation of function signatures
//! (parameters with type info, mutability, annotations) and produces
//! a set of SAW-level constraints.

use anyhow::Result;

/// A function parameter with all known constraints from the type system.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    pub name: String,
    pub ty: TypeInfo,
    pub mutability: Mutability,
    pub nullable: Nullability,
    pub annotations: Vec<Annotation>,
}

/// A function with its full signature and constraint info.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionInfo {
    pub name: String,
    pub mangled_name: Option<String>,
    pub params: Vec<ParamInfo>,
    pub return_type: TypeInfo,
    pub can_throw: bool,
    pub annotations: Vec<Annotation>,
}

/// Language-independent type representation.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeInfo {
    /// Signed integer with bit width
    SignedInt(u32),
    /// Unsigned integer with bit width
    UnsignedInt(u32),
    /// Boolean (1-bit)
    Bool,
    /// Byte array of known size
    ByteArray(usize),
    /// Pointer/reference to another type
    Pointer(Box<TypeInfo>),
    /// Struct with named fields
    Struct {
        name: String,
        size_bytes: Option<usize>,
        fields: Vec<(String, TypeInfo)>,
    },
    /// Enum with known variants and discriminant range
    Enum {
        name: String,
        variants: Vec<String>,
        discriminant_bits: u32,
    },
    /// Option<T> -- None or Some(T)
    Option(Box<TypeInfo>),
    /// Result<T, E> -- Ok(T) or Err(E)
    Result(Box<TypeInfo>, Box<TypeInfo>),
    /// Opaque type with known size
    Opaque { name: String, size_bytes: usize },
    /// Void / unit type
    Void,
}

/// Whether a parameter can be modified by the function.
#[derive(Debug, Clone, PartialEq)]
pub enum Mutability {
    /// Immutable
    Readonly,
    /// Mutable
    Mutable,
    /// Write-only
    WriteOnly,
}

/// Whether a pointer/reference can be null.
#[derive(Debug, Clone, PartialEq)]
pub enum Nullability {
    /// Never null
    NonNull,
    /// May be null
    Nullable,
}

/// Annotations from the source language.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    /// SAL: _In_reads_(n)
    InReads(usize),
    /// SAL: _Out_writes_(n)
    OutWrites(usize),
    /// SAL: _Inout_ — read on entry, may be modified
    Inout,
    /// SAL: _Pre_valid_ — pointer is valid on entry
    PreValid,
    /// SAL: _Post_invalid_ — pointer may be invalid after call
    PostInvalid,
    /// SAL: _Must_inspect_result_
    MustInspectResult,
    /// LLVM: noalias
    NoAlias,
    /// LLVM: nocapture
    NoCapture,
    /// LLVM: dereferenceable(n)
    Dereferenceable(usize),
    /// C++: noexcept / LLVM: nounwind
    NoThrow,
    /// Custom annotation
    Custom(String),
}

/// A generated SAW spec constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecConstraint {
    pub function_name: String,
    pub mangled_name: Option<String>,
    pub params: Vec<ParamConstraint>,
    pub return_constraint: ReturnConstraint,
    pub can_throw: bool,
    pub postconditions: Vec<String>,
}

/// Constraint on a single parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamConstraint {
    pub name: String,
    pub alloc_type: AllocType,
    pub saw_type: String,
    pub preconditions: Vec<String>,
    pub unchanged_after: bool,
}

/// How to allocate the parameter in the SAW spec.
#[derive(Debug, Clone, PartialEq)]
pub enum AllocType {
    AllocReadonly,
    AllocMutable,
    FreshVar,
}

/// Constraint on the return value.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnConstraint {
    pub saw_type: String,
    pub value_constraints: Vec<String>,
}

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

        let saw_type = type_to_saw(inner_ty);

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
                    preconditions
                        .push(format!("// _In_reads_({n}) -- readable buffer of {n} elements"));
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
                    preconditions
                        .push("// nocapture -- pointer not retained after call".into());
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
        });
    }

    let return_constraint = derive_return_constraint(&func.return_type);

    // Function-level annotation handling
    for ann in &func.annotations {
        match ann {
            Annotation::MustInspectResult => {
                postconditions.push(
                    "// _Must_inspect_result_ -- return value must be checked".into(),
                );
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
        postconditions,
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

    ReturnConstraint {
        saw_type: type_to_saw(ty),
        value_constraints,
    }
}

pub fn type_to_saw(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Bool => "llvm_int 1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("llvm_int {bits}"),
        TypeInfo::ByteArray(n) => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Pointer(inner) => type_to_saw(inner),
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Struct { name, .. } => format!("llvm_alias \"{name}\""),
        TypeInfo::Enum { name, .. } => format!("llvm_alias \"{name}\""),
        TypeInfo::Option(inner) => format!("// Option<{}>", type_to_saw(inner)),
        TypeInfo::Result(ok, err) => {
            format!("// Result<{}, {}>", type_to_saw(ok), type_to_saw(err))
        }
        TypeInfo::Opaque { name, size_bytes } => {
            if *size_bytes > 0 {
                format!("llvm_array {size_bytes} (llvm_int 8)")
            } else {
                format!("llvm_alias \"{name}\"")
            }
        }
        TypeInfo::Void => "// void".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_to_saw_scalars() {
        assert_eq!(type_to_saw(&TypeInfo::Bool), "llvm_int 1");
        assert_eq!(type_to_saw(&TypeInfo::SignedInt(32)), "llvm_int 32");
        assert_eq!(type_to_saw(&TypeInfo::UnsignedInt(64)), "llvm_int 64");
    }

    #[test]
    fn test_type_to_saw_byte_array() {
        assert_eq!(
            type_to_saw(&TypeInfo::ByteArray(16)),
            "llvm_array 16 (llvm_int 8)"
        );
    }

    #[test]
    fn test_type_to_saw_struct_with_size() {
        let ty = TypeInfo::Struct {
            name: "MyStruct".into(),
            size_bytes: Some(24),
            fields: vec![],
        };
        assert_eq!(type_to_saw(&ty), "llvm_array 24 (llvm_int 8)");
    }

    #[test]
    fn test_type_to_saw_struct_named() {
        let ty = TypeInfo::Struct {
            name: "MyStruct".into(),
            size_bytes: None,
            fields: vec![("x".into(), TypeInfo::SignedInt(32))],
        };
        assert_eq!(type_to_saw(&ty), "llvm_alias \"MyStruct\"");
    }

    #[test]
    fn test_type_to_saw_enum() {
        let ty = TypeInfo::Enum {
            name: "Status".into(),
            variants: vec!["Ok".into(), "Err".into(), "Pending".into()],
            discriminant_bits: 64,
        };
        assert_eq!(type_to_saw(&ty), "llvm_alias \"Status\"");
    }

    #[test]
    fn test_type_to_saw_option() {
        let ty = TypeInfo::Option(Box::new(TypeInfo::SignedInt(32)));
        assert_eq!(type_to_saw(&ty), "// Option<llvm_int 32>");
    }

    #[test]
    fn test_type_to_saw_result() {
        let ty = TypeInfo::Result(
            Box::new(TypeInfo::UnsignedInt(8)),
            Box::new(TypeInfo::SignedInt(32)),
        );
        assert_eq!(type_to_saw(&ty), "// Result<llvm_int 8, llvm_int 32>");
    }

    #[test]
    fn test_type_to_saw_opaque_with_size() {
        let ty = TypeInfo::Opaque {
            name: "Unknown".into(),
            size_bytes: 32,
        };
        assert_eq!(type_to_saw(&ty), "llvm_array 32 (llvm_int 8)");
    }

    #[test]
    fn test_type_to_saw_opaque_no_size() {
        let ty = TypeInfo::Opaque {
            name: "std::string".into(),
            size_bytes: 0,
        };
        assert_eq!(type_to_saw(&ty), "llvm_alias \"std::string\"");
    }

    #[test]
    fn test_type_to_saw_pointer() {
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32)));
        assert_eq!(type_to_saw(&ty), "llvm_int 32");
    }

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
            annotations: vec![Annotation::MustInspectResult],
        };
        let specs = derive_constraints(&[func]).unwrap();
        assert!(specs[0]
            .postconditions
            .iter()
            .any(|p| p.contains("Must_inspect_result")));
    }
}
