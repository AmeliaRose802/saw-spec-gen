//! Shared orchestration for the legacy and opt-in typed override APIs.

use crate::constraints::container_layouts::ContainerCatalog;
use crate::constraints::GlobalVarInfo;
use crate::parsers::llvm_ir::struct_defs;
use crate::transform::extern_override_scan::{self, OverrideTarget};

use super::{emit_overrides, global_width_bits, EmittedBitcodeOverrides, FunctionalLayouts};

pub(super) fn scan_and_emit_with(
    llvm_ir_path: Option<&std::path::Path>,
    target_symbol: &str,
    already_covered: &[String],
    all_globals: &[GlobalVarInfo],
    container_catalog: &ContainerCatalog,
    scan: fn(&str, &str) -> Vec<OverrideTarget>,
) -> EmittedBitcodeOverrides {
    let Some(path) = llvm_ir_path else {
        return EmittedBitcodeOverrides::empty();
    };
    let ir_text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: could not re-read LLVM IR for extern override scan: {e}");
            return EmittedBitcodeOverrides::empty();
        }
    };
    let targets = scan(&ir_text, target_symbol);
    // Restrict global-clobber to globals that the IR actually declares
    // as `global` (not `constant`) and whose width we can express as an
    // `llvm_int N` term. Each `OverrideTarget` carries its own
    // `globals_written` set (computed by `extern_override_scan::scan`):
    //   - DeclareOnly targets conservatively list all externally-visible
    //     mutable globals (any opaque callee could write them).
    //   - Defined bodies (UsesVarargsIntrinsic) list exactly the globals
    //     their transitive call-chain stores to.
    // `emit_one` filters `mutable_globals` against that set.
    let mg = extern_override_scan::scan_mutable_globals(&ir_text);
    let mutable_globals: Vec<GlobalVarInfo> = all_globals
        .iter()
        .filter(|g| mg.all.contains(g.mangled_name.as_str()))
        .filter(|g| global_width_bits(&g.ty).is_some())
        .cloned()
        .collect();
    // Pre-discover container layouts so the functional STL emitter can
    // dispatch on canonical method names. The discovery is gated by
    // the AST-derived `ContainerCatalog` (saw_spec_gen-qms): we only
    // emit a functional override for a container whose shape the
    // catalog has independently confirmed from the clang AST. This
    // means the catalog — not ad-hoc IR-string matching — is the
    // source of truth for which containers we model.
    let struct_table = struct_defs(&ir_text);
    let layouts = FunctionalLayouts::discover(&struct_table, container_catalog);
    let emitted = emit_overrides(&targets, already_covered, &mutable_globals, &layouts);
    if !emitted.is_empty() {
        eprintln!(
            "Bitcode override scan: emitting {} extern override(s)",
            emitted.override_names.len(),
        );
    }
    emitted
}
