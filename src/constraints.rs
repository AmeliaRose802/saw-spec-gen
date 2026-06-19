//! Core constraint derivation logic.
//!
//! Takes a language-independent representation of function signatures
//! (parameters with type info, mutability, annotations) and produces
//! a set of SAW-level constraints.
//!
//! The module is split into three child modules to keep each file small
//! and focused:
//!
//! - [`types`]: language-independent data types (function/parameter info,
//!   type representation, derived constraint shapes).
//! - [`derive`]: turns [`types::FunctionInfo`] values into
//!   [`types::SpecConstraint`] values.
//! - [`saw_type`]: maps a [`types::TypeInfo`] into the SAW spec type
//!   string (e.g. `llvm_int 32`, `llvm_array N (llvm_int 8)`).
//!
//! For backward compatibility everything is re-exported flat at
//! `crate::constraints::*`.

pub mod container_layouts;
pub mod container_layouts_derive;
pub mod derive;
pub mod length_binding;
pub mod length_companion;
pub mod saw_type;
pub mod struct_shape_recognizer;
pub mod types;
pub mod value_clauses;

pub use derive::{correct_sret_from_ir, derive_constraints};
pub use saw_type::{pointee_saw_type, type_to_saw};
pub use types::{
    AllocType, Annotation, CalledFunction, EnumVariant, FunctionInfo, GlobalVarInfo, Mutability,
    Nullability, ParamConstraint, ParamInfo, ReturnConstraint, SpecConstraint, TypeInfo,
};
pub use value_clauses::value_clauses;
