//! Emit SAW verification scripts from derived constraints.
//!
//! Generates `.saw` files SAW can load to create override specs for
//! unspecified functions plus complete verification scripts. The module
//! is organized into focused submodules:
//!
//! - [`writer`] — shared `VOID_SAW_TYPE` constant and the
//!   `is_void_saw_type` predicate used to short-circuit return-binding
//!   in the various spec emitters.
//! - [`names`] — identifier sanitization (`sanitize_name`,
//!   `spec_safe_id`, `stub_function_name`).
//! - [`types`] — SAW LLVM / MIR / LLVM-IR type-string mapping.
//! - [`path_utils`] — relative-path computation for `include` directives.
//! - [`llvm_spec`] — LLVM-mode spec emission + the `EmitMode` enum +
//!   `emit_saw_specs_with_mode` dispatcher.
//! - [`mir_spec`] — MIR-mode spec emission for Rust.
//! - [`havoc`] — adversarial spec generation for virtual / external
//!   methods.
//! - [`stubs`] — `vtable_stubs.ll` + `vtable_stubs.bc` generation plus
//!   the `AssembledStubs` status enum.
//! - [`overrides`] — `interface_overrides.saw`, per-class helpers,
//!   `FieldKind`-aware container layouts.
//! - [`factory`] — interface-factory spec generation.
//! - [`verify_script`] — top-level `verify.saw` orchestration.

mod factory;
mod havoc;
mod havoc_params;
mod llvm_return;
mod llvm_setup;
mod llvm_spec;
mod mir_spec;
mod names;
mod overrides;
mod path_utils;
pub mod stl_overrides;
mod stubs;
mod types;
mod verify_script;
mod verify_script_close;
mod verify_script_steps;
mod vtable_ir;
mod writer;

// Cross-emitter helpers (Cryptol/LLVM bridge) used by both this
// module's C++ generator and the standalone Rust generator in
// `crate::gen_verify_rust`. Public so the Rust path can reuse it
// without a circular dep on `verify_script_steps`.
mod bitcode_overrides;
mod bitcode_overrides_functional;
mod bitcode_overrides_functional_string;
mod bitcode_overrides_functional_vector;
pub mod cryptol_bridge;
pub mod cryptol_sig_parse;

// Re-export the public surface that the rest of the crate depends on.
// `allow(unused_imports)` is appropriate here because this is a binary
// crate — not every re-export is consumed by `main.rs`, but the items
// remain part of the module's public surface for integration tests and
// future extractors.

#[allow(unused_imports)]
pub use bitcode_overrides::{
    emit_overrides as emit_bitcode_overrides, scan_and_emit as scan_and_emit_bitcode_overrides,
    EmittedBitcodeOverrides,
};
#[allow(unused_imports)]
pub use factory::emit_interface_factory_spec;
#[allow(unused_imports)]
pub use llvm_spec::{
    emit_operator_new_spec, emit_saw_specs, emit_saw_specs_with_globals,
    emit_single_experimental_spec, EmitMode,
};
#[allow(unused_imports)]
pub use mir_spec::emit_mir_saw_specs;
#[allow(unused_imports)]
pub use stubs::{
    assemble_vtable_stubs, emit_interface_stubs, link_stubs_with_main, AssembledStubs,
};
#[allow(unused_imports)]
pub use verify_script::emit_verification_script;
