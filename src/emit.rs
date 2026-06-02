//! Backend emitters: produce SAW spec scripts, Cryptol constraint
//! sketches, and Rust trait stubs from
//! [`crate::constraints::SpecConstraint`] values.

pub mod cryptol_emit;
pub mod rust_trait_emit;
pub mod saw_emit;
