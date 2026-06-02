//! Parse and traverse clang `-ast-dump=json` output.
//!
//! The module exposes a small, focused public API; the heavy lifting
//! lives in submodules organized by concern:
//!
//! - [`node`] — strongly-typed [`AstNode`] struct (serde-deserialized).
//! - [`parse`] — file loading, JSON splitting, AST merging.
//! - [`visitor`] — generic pre/post-order [`Visitor`] trait + walker.
//! - [`type_ctx`] — class/enum/layout context built in one AST pass.
//! - [`cpp_types`] — qualType string parsing + STL/Win32 size tables.
//! - [`functions`] — [`extract_functions`] and parameter parsing.
//! - [`virtual_methods`] — virtual method + virtual destructor detection.
//! - [`constructors`] — constructor extraction + IR-symbol filtering.
//! - [`globals`] — file-scope variable + reference resolution.
//! - [`calls`] — `called_functions` resolution pass.
//! - [`interfaces`] — missing-interface-type detection.
//! - [`enum_bits`] — whole-AST enum width collection for post-processing.
//! - [`sal`] — `AnnotateAttr` (SAL macro) parsing.
//! - [`source_cache`] — source-file cache backing SAL recovery.
//! - [`system_headers`] — path heuristic for vendor/SDK headers.
//!
//! ## Usage
//!
//! ```no_run
//! use saw_spec_gen::clang_ast;
//! let ast = clang_ast::parse_ast(std::path::Path::new("ast.json"))?;
//! let funcs = clang_ast::extract_functions(&ast, None)?;
//! # Ok::<(), anyhow::Error>(())
//! ```

mod calls;
mod constructors;
mod cpp_types;
mod enum_bits;
mod functions;
mod globals;
mod interfaces;
mod node;
mod parse;
mod path_filter;
mod sal;
mod source_cache;
mod system_headers;
mod type_ctx;
mod virtual_methods;
mod visitor;

// Re-export the public surface that the rest of the crate (and tests)
// depend on. The shape matches the original monolithic module so callers
// do not need to be updated beyond a type change for the AST root.
//
// `allow(unused_imports)` is appropriate here because this is a binary
// crate: not every re-export is consumed by `main.rs`, but the items
// are part of the public surface used by the integration test suite
// and downstream extractors.

#[allow(unused_imports)]
pub use constructors::{extract_constructors, filter_ctors_by_ir_symbols, ClassConstructor};
#[allow(unused_imports)]
pub use cpp_types::{cpp_type_size_align, lookup_known_type_size};
#[allow(unused_imports)]
pub use enum_bits::collect_all_enum_bits;
#[allow(unused_imports)]
pub use functions::extract_functions;
#[allow(unused_imports)]
pub use globals::extract_all_globals;
#[allow(unused_imports)]
pub use interfaces::{detect_missing_interfaces, MissingInterfaceRef};
#[allow(unused_imports)]
pub use node::AstNode;
#[allow(unused_imports)]
pub use parse::{merge_asts, parse_ast, MAX_AST_FILE_SIZE};
#[allow(unused_imports)]
pub use path_filter::{filter_ast_file, filter_translation_unit_value, FilterStats};
#[allow(unused_imports)]
pub use virtual_methods::{classes_with_virtual_dtor, extract_virtual_methods, InterfaceMethod};

// Visitor / typed-node primitives, exposed for downstream tooling and
// future extractors.
#[allow(unused_imports)]
pub use visitor::{walk, ClassStack, Visitor, WalkAction};
