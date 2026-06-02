//! Post-processing transforms run between parsing and emission:
//! alias-resolution fallbacks, spec text rewriting, and SAW type
//! lookups.

pub mod alias_fallbacks;
pub mod alias_fallbacks_ir;
pub mod eh_globals;
pub mod extern_override_scan;
pub(crate) mod ir_globals;
pub mod patch_llvm_ir;
pub mod spec_rewrite;
pub mod type_resolve;
