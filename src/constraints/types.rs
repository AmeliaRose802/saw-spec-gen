//! Data types shared across the constraint derivation pipeline.
//!
//! These are the language-independent representations of function
//! signatures, parameters, types, and the derived SAW constraints.
//! No derivation logic lives here — see [`super::derive`] for that.

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
    pub is_virtual: bool,
    /// Whether the function has a body (definition) or is declaration-only (external).
    pub has_body: bool,
    /// True if this function is declared in a system header (cstdio, ucrt, etc.).
    /// We treat system functions as external for verification purposes — their
    /// inline wrappers typically call LLVM intrinsics (va_start, etc.) and
    /// platform-specific symbols that SAW can't resolve.
    pub is_system: bool,
    pub annotations: Vec<Annotation>,
    /// Global variables referenced by this function.
    pub referenced_globals: Vec<GlobalVarInfo>,
    /// Functions called by this function (mangled names).
    pub called_functions: Vec<CalledFunction>,
}

/// A function called from another function's body.
#[derive(Debug, Clone, PartialEq)]
pub struct CalledFunction {
    pub name: String,
    pub mangled_name: String,
    pub has_body: bool,
}

/// A global variable referenced by a function.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalVarInfo {
    pub name: String,
    pub mangled_name: String,
    pub ty: TypeInfo,
    /// Initial value as a string (e.g. "7" for int super_important = 7)
    pub init_value: Option<String>,
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
    pub is_virtual: bool,
    /// Whether the function has a body (definition) or is declaration-only.
    pub has_body: bool,
    pub postconditions: Vec<String>,
    /// Global variables referenced by this function.
    pub referenced_globals: Vec<GlobalVarInfo>,
}

/// Constraint on a single parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamConstraint {
    pub name: String,
    pub alloc_type: AllocType,
    pub saw_type: String,
    pub preconditions: Vec<String>,
    pub unchanged_after: bool,
    /// Byte size derived from an LLVM `dereferenceable(N)` annotation on the
    /// parameter, if any.  When the AST gives us only an opaque struct name
    /// and no LLVM IR is supplied via `--llvm-ir`, this size is the best
    /// fallback the tool has for substituting `llvm_alias "Foo"` with a
    /// concrete `llvm_array N (llvm_int 8)`.  None when no `dereferenceable`
    /// annotation was present on the parameter.
    pub dereferenceable_size: Option<usize>,
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
    /// True when the return is a struct that LLVM lowers via sret pointer.
    pub is_sret: bool,
    /// True when the function returns a pointer (e.g. operator new → void*).
    /// The spec should use llvm_alloc + llvm_return for the return value.
    pub returns_pointer: bool,
}
