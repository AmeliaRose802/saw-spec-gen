//! Parse mir-json output and extract function signatures with Rust
//! type info.
//!
//! mir-json serializes Rust MIR to JSON, preserving full type
//! information including enum variants, reference mutability,
//! lifetimes, etc. The module is organized into focused submodules:
//!
//! - [`node`] — typed serde structs for the JSON shape.
//! - [`parse`] — streamed file I/O with a 2 GiB sanity guard.
//! - [`adt`] — `name → TypeInfo` lookup table construction.
//! - [`rust_types`] — Rust type-string parser
//!   (`"ty::Ref<'_, u32, Shared>"` → `TypeInfo::Pointer(...)`).
//! - [`functions`] — top-level `extract_functions` entry point.

mod adt;
mod functions;
mod node;
mod parse;
mod rust_types;

// `allow(unused_imports)` because saw-spec-gen is a binary crate —
// some items remain part of the module's public surface for tests and
// downstream extractors even when `main.rs` doesn't reach for them
// directly.

#[allow(unused_imports)]
pub use functions::extract_functions;
#[allow(unused_imports)]
pub use node::MirJson;
#[allow(unused_imports)]
pub use parse::{parse_mir, MAX_MIR_FILE_SIZE};
