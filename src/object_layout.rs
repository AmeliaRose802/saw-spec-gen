//! Compiler-grounded C++ object layouts. Unknown layout facts are never sizes.

pub mod bitcode;
pub mod capture;
pub mod clang;
pub mod config_keys;
pub mod data_layout;
pub mod derive;
pub mod emit;
mod emit_bitfields;
pub mod enums;
pub mod model;
pub mod plan;
pub mod validate;

pub use model::*;
