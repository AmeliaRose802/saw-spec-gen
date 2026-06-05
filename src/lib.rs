//! Library entry point for `saw-spec-gen`.
//!
//! All real logic lives here so that `cargo test --lib` has a target to
//! build. `src/main.rs` is a thin binary that parses CLI args and
//! dispatches into `commands::*` defined below.

// Top-level utilities that stay at the crate root.
pub mod buffer_overrides;
pub mod collect_results;
pub mod commands;
pub mod constraints;
pub mod dump_types;
pub mod gen_verify;
pub mod gen_verify_helpers;
pub mod gen_verify_rust;
pub mod mangle;

// Grouped subsystems. Each is a folder under `src/` with its own
// module root file. Re-exported below so existing `crate::clang_ast::`
// (etc.) call sites keep working without churn.
pub mod emit;
pub mod parsers;
pub mod transform;

// Re-exports keep the flat-from-crate-root view: the rest of the crate
// can still write `crate::clang_ast::parse_ast(...)` rather than
// `crate::parsers::clang_ast::parse_ast(...)`. The folder grouping is
// a layout-only change.
pub use emit::{cryptol_emit, rust_trait_emit, saw_emit};
pub use parsers::{clang_ast, cryptol_sig, llvm_ir, mir_json};
pub use transform::{
    alias_fallbacks, alias_fallbacks_ir, patch_llvm_ir, spec_rewrite, type_resolve,
};
