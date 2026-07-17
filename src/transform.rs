//! Post-processing transforms run between parsing and emission:
//! alias-resolution fallbacks, spec text rewriting, and SAW type
//! lookups.

pub mod alias_fallbacks;
pub mod alias_fallbacks_ir;
pub mod crucible_safety;
pub mod eh_globals;
pub mod extern_override_scan;
pub(crate) mod ir_globals;
pub(crate) mod memcmp_scan;
pub mod patch_llvm_ir;
pub mod spec_rewrite;
pub mod type_resolve;
