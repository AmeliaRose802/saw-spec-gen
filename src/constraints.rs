//! Core constraint derivation logic.
//!
//! Takes a language-independent representation of function signatures
//! (parameters with type info, mutability, annotations) and produces
//! a set of SAW-level constraints.

use anyhow::Result;

/// A function parameter with all known constraints from the type system.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub ty: TypeInfo,
    pub mutability: Mutability,
    pub nullable: Nullability,
    pub annotations: Vec<Annotation>,
}

/// A function with its full signature and constraint info.
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub name: String,
    pub mangled_name: Option<String>,
    pub params: Vec<ParamInfo>,
    pub return_type: TypeInfo,
    pub can_throw: bool,
    pub annotations: Vec<Annotation>,
}

/// Language-independent type representation.
#[derive(Debug, Clone)]
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
    /// Option<T> — None or Some(T)
    Option(Box<TypeInfo>),
    /// Result<T, E> — Ok(T) or Err(E)
    Result(Box<TypeInfo>, Box<TypeInfo>),
    /// Opaque type with known size
    Opaque { name: String, size_bytes: usize },
    /// Void / unit type
    Void,
}

/// Whether a parameter can be modified by the function.
#[derive(Debug, Clone, PartialEq)]
pub enum Mutability {
    /// Immutable — function MUST NOT modify this
    /// Sources: Rust &T, C++ const&, LLVM readonly, SAL _In_
    Readonly,
    /// Mutable — function MAY modify this
    /// Sources: Rust &mut T, C++ non-const ref, SAL _Inout_
    Mutable,
    /// Write-only — function MUST write before returning
    /// Sources: SAL _Out_
    WriteOnly,
}

/// Whether a pointer/reference can be null.
#[derive(Debug, Clone, PartialEq)]
pub enum Nullability {
    /// Never null — guaranteed by type system
    /// Sources: Rust references, LLVM nonnull
    NonNull,
    /// May be null
    /// Sources: raw pointers, SAL _Ret_maybenull_
    Nullable,
}

/// Annotations from the source language.
#[derive(Debug, Clone)]
pub enum Annotation {
    /// SAL: _In_reads_(n)
    InReads(usize),
    /// SAL: _Out_writes_(n)
    OutWrites(usize),
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
#[derive(Debug, Clone)]
pub struct SpecConstraint {
    pub function_name: String,
    pub mangled_name: Option<String>,
    pub params: Vec<ParamConstraint>,
    pub return_constraint: ReturnConstraint,
    pub can_throw: bool,
    pub postconditions: Vec<String>,
}

/// Constraint on a single parameter.
#[derive(Debug, Clone)]
pub struct ParamConstraint {
    pub name: String,
    pub alloc_type: AllocType,
    pub saw_type: String,
    pub preconditions: Vec<String>,
    pub unchanged_after: bool,
}

/// How to allocate the parameter in the SAW spec.
#[derive(Debug, Clone)]
pub enum AllocType {
    AllocReadonly,
    AllocMutable,
    FreshVar,
}

/// Constraint on the return value.
#[derive(Debug, Clone)]
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

    for param in &func.params {
        let (alloc_type, unchanged_after) = match param.mutability {
            Mutability::Readonly => (AllocType::AllocReadonly, true),
            Mutability::Mutable => (AllocType::AllocMutable, false),
            Mutability::WriteOnly => (AllocType::AllocMutable, false),
        };

        let saw_type = type_to_saw(&param.ty);
        let mut preconditions = Vec::new();

        // Add type-level value constraints
        match &param.ty {
            TypeInfo::Bool => {
                // No constraint needed — i1 is inherently 0 or 1
            }
            TypeInfo::Enum { variants, discriminant_bits, .. } => {
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

        // Add annotation-driven constraints
        for ann in &param.annotations {
            match ann {
                Annotation::Dereferenceable(n) => {
                    preconditions.push(format!("// dereferenceable({n}) bytes"));
                }
                Annotation::NoAlias => {
                    preconditions.push("// noalias — does not alias other params".into());
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

    Ok(SpecConstraint {
        function_name: func.name.clone(),
        mangled_name: func.mangled_name.clone(),
        params,
        return_constraint,
        can_throw: func.can_throw,
        postconditions,
    })
}

fn derive_return_constraint(ty: &TypeInfo) -> ReturnConstraint {
    let mut value_constraints = Vec::new();

    match ty {
        TypeInfo::Bool => {
            // i1 — no extra constraint needed
        }
        TypeInfo::Enum { variants, discriminant_bits, .. } => {
            let max_disc = variants.len() as u64 - 1;
            value_constraints.push(format!(
                "llvm_postcond {{{{ ret_disc <= ({max} : [{bits}]) }}}}",
                max = max_disc,
                bits = discriminant_bits,
            ));
        }
        TypeInfo::Option(_) => {
            value_constraints.push(
                "llvm_postcond {{ ret_disc <= (1 : [8]) }}".into(),
            );
        }
        TypeInfo::UnsignedInt(bits) => {
            // Unsigned — no negative values, inherent in the type
            value_constraints.push(format!("// unsigned {bits}-bit — no extra constraint"));
        }
        _ => {}
    }

    ReturnConstraint {
        saw_type: type_to_saw(ty),
        value_constraints,
    }
}

fn type_to_saw(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Bool => "llvm_int 1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("llvm_int {bits}"),
        TypeInfo::ByteArray(n) => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Pointer(inner) => type_to_saw(inner),
        TypeInfo::Struct { size_bytes: Some(n), .. } => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Struct { name, .. } => format!("llvm_alias \"{name}\""),
        TypeInfo::Enum { name, .. } => format!("llvm_alias \"{name}\""),
        TypeInfo::Option(inner) => format!("// Option<{}>", type_to_saw(inner)),
        TypeInfo::Result(ok, err) => {
            format!("// Result<{}, {}>", type_to_saw(ok), type_to_saw(err))
        }
        TypeInfo::Opaque { size_bytes, .. } => format!("llvm_array {size_bytes} (llvm_int 8)"),
        TypeInfo::Void => "// void".into(),
    }
}
