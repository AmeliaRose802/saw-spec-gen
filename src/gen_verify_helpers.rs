//! Helpers extracted from [`super::gen_verify`] to stay under the
//! 500-non-whitespace-line limit.

use crate::clang_ast;
use crate::saw_emit;
use anyhow::Result;
use std::path::Path;

/// Warn (on stderr) about interfaces referenced by class fields but
/// missing from the merged clang AST. A missing interface makes
/// `extract_virtual_methods` skip its vtable stubs, so the generated
/// spec fails to verify (the indirect calls have no overrides).
/// Extracted from [`super::gen_verify::run`] to keep that file under
/// the line limit.
pub(crate) fn warn_missing_interfaces(parsed_ast: &clang_ast::AstNode) {
    let missing = clang_ast::detect_missing_interfaces(parsed_ast);
    if missing.is_empty() {
        return;
    }
    eprintln!(
        "warning: {} interface(s) referenced by class fields but missing from AST(s):",
        missing.len(),
    );
    for m in &missing {
        eprintln!(
            "  - {}::{} : {}<{}> (interface AST not provided)",
            m.owning_class, m.field_name, m.wrapper, m.interface_name,
        );
    }
    eprintln!("  hint: pass additional --ast files containing each missing interface so");
    eprintln!("        gen-verify can synthesize vtable stubs for their virtual methods.");
}

/// Assemble `vtable_stubs.ll` → `.bc` and optionally pre-link with the
/// main bitcode. Extracted from [`super::gen_verify::run`] to keep that
/// file under the line limit.
pub(crate) fn assemble_and_link_stubs(
    has_interfaces: bool,
    use_llvm_combine_modules: bool,
    bitcode: &Path,
    output: &Path,
) -> saw_emit::AssembledStubs {
    if !has_interfaces {
        return saw_emit::AssembledStubs::NoStubs;
    }
    let assembled = saw_emit::assemble_vtable_stubs(output);
    match &assembled {
        saw_emit::AssembledStubs::Bitcode {
            bc_filename,
            assembler,
        } => {
            eprintln!("Assembled vtable stubs to {bc_filename} via `{assembler}`");
        }
        saw_emit::AssembledStubs::TextOnly { ll_filename } => {
            let ll = output.join(ll_filename).display().to_string();
            let bc = format!("{}/vtable_stubs.bc", output.display());
            eprintln!(
                "warning: no llvm-as / clang on PATH — {ll_filename} not assembled.\n\
                 Run: llvm-as {ll} -o {bc}\n  or: clang -c -emit-llvm {ll} -o {bc}",
            );
        }
        saw_emit::AssembledStubs::LinkedBitcode { .. } | saw_emit::AssembledStubs::NoStubs => {}
    }
    if use_llvm_combine_modules {
        return assembled;
    }
    let linked = saw_emit::link_stubs_with_main(bitcode, output, assembled);
    match &linked {
        saw_emit::AssembledStubs::LinkedBitcode {
            combined_filename,
            linker,
        } => {
            eprintln!(
                "Pre-linked main + vtable stubs into {combined_filename} via `{linker}` \
                 (verify.saw will not need llvm_combine_modules).",
            );
        }
        saw_emit::AssembledStubs::Bitcode { .. } => {
            eprintln!(
                "warning: llvm-link not found on PATH; falling back to \
                 llvm_combine_modules in the emitted script. Stock SAW \
                 v1.5 will not be able to run that — install llvm-link \
                 (ships with LLVM) or pass --use-llvm-combine-modules \
                 to silence this warning.",
            );
        }
        _ => {}
    }
    linked
}

/// Soft-exit path used when `--spec-only-on-missing` is set and the
/// target function has no implementation we can hook into. Writes a
/// `result.json` that pretty-specs' `adapt-saw-results` will pick up
/// and classify as `not_attempted` with a human-readable reason, then
/// returns Ok so the pipeline doesn't go red on Cryptol-only helpers
/// (e.g. `packPad`, `derivePin`, etc.) that have no C++ analog by
/// design.
pub(crate) fn emit_spec_only_result(
    output: &Path,
    cryptol_fn: &str,
    function: &str,
    reason: &str,
) -> Result<()> {
    crate::verify_result::write_spec_only_result(output, "cpp", function, cryptol_fn, reason)?;
    eprintln!(
        "spec-only: no implementation for '{}'; wrote {}",
        function,
        output.join("result.json").display(),
    );
    Ok(())
}
