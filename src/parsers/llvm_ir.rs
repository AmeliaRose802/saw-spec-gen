//! Parse LLVM IR text (`.ll` files) and extract function signatures.
//!
//! This provides a language-agnostic way to extract type information
//! from any source that compiles to LLVM IR — works with output from
//! `llvm-dis` (which converts `.bc` to `.ll`).
//!
//! The module is organized into focused submodules:
//!
//! - [`tokens`] — generic paren/brace-aware string splitters.
//! - [`struct_types`] — LLVM struct definitions + size/alignment math.
//! - [`type_parser`] — `TypeInfo` resolution from type strings.
//! - [`attrs`] — typed accumulator for parameter attributes
//!   (`readonly`, `sret(...)`, `dereferenceable(N)`, etc.).
//! - [`params`] — per-parameter parsing using [`attrs::IrParamAttrs`].
//! - [`function_sig`] — `declare`/`define` line parsing + sret
//!   rewriting + return-type extraction.
//! - [`load`] — file I/O with OOM guard.

mod attrs;
pub mod callgraph;
mod function_sig;
mod load;
mod params;
mod struct_types;
mod tokens;
mod type_parser;

// Public surface. `allow(unused_imports)` because saw-spec-gen is a
// binary crate — not every re-export is consumed from `main.rs`, but
// the items remain part of the module's documented surface for
// integration tests and downstream extractors.

#[allow(unused_imports)]
pub use function_sig::extract_functions;
#[allow(unused_imports)]
pub use load::{load_optional, parse_llvm_ir, read_target_triple, MAX_IR_FILE_SIZE};
#[allow(unused_imports)]
pub use struct_types::struct_sizes;
