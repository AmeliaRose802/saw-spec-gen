//! Helpers extracted from [`super::gen_verify`] to stay under the
//! 500-non-whitespace-line limit.

use crate::alias_fallbacks::{apply_cli_overrides, dump_fallback_diagnostics};
use crate::constraints::{FunctionInfo, GlobalVarInfo};
use crate::spec_rewrite::{apply_alias_rewrites_protected, collect_type_sizes};
use crate::{clang_ast, llvm_ir, saw_emit};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Gather uninterpreted-primitive contracts (`@uninterpreted` annotations
/// and `[[uninterpreted]]` config) together with auto-discovered
/// compositional contracts for declare-only cross-TU callees, then emit
/// the combined `llvm_unsafe_assume_spec` block. Extracted from
/// [`super::gen_verify::run`] to keep that file under the line limit.
pub(crate) fn gather_and_emit_uninterpreted(
    cryptol_spec: &Path,
    uninterpreted_cfg: &[crate::uninterpreted::UninterpretedEntry],
    ir_text: &str,
    target_symbol: &str,
    all_functions: &[crate::constraints::FunctionInfo],
) -> crate::uninterpreted::UninterpretedBlock {
    let entries = crate::uninterpreted::gather(
        cryptol_spec,
        uninterpreted_cfg,
        ir_text,
        target_symbol,
        all_functions,
    );
    crate::uninterpreted::emit_uninterpreted_block(&entries, cryptol_spec)
}

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

/// Collect AST and IR globals, preserving static and exception-lowering initializers.
pub(crate) fn collect_globals(
    parsed_ast: &clang_ast::AstNode,
    llvm_ir_path: Option<&Path>,
) -> Result<(Vec<GlobalVarInfo>, HashMap<String, llvm_ir::IrStructDef>)> {
    let mut all_globals = clang_ast::extract_all_globals(parsed_ast)?;

    // Augment with mutable globals discovered in the LLVM IR that the
    // clang AST parser missed (function-local statics, compiler-
    // generated globals, etc.). Without this, SAW aborts with
    // "Global symbol not allocated" when symbolically executing a body
    // that touches an IR-only global.
    let ir_struct_defs = if let Some(ir_path) = llvm_ir_path {
        if let Ok(ir_text) = std::fs::read_to_string(ir_path) {
            let extra =
                crate::transform::ir_globals::discover_ir_only_globals(&ir_text, &all_globals);
            if !extra.is_empty() {
                eprintln!(
                    "  discovered {} IR-only mutable global(s) not in clang AST",
                    extra.len(),
                );
                all_globals.extend(extra);
            }
            crate::transform::ir_globals::mark_static_initializers(&mut all_globals, &ir_text);
            llvm_ir::struct_defs(&ir_text)
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    // Inject the exception-lower bookkeeping globals (@__exclow_error_*)
    // with the right TypeInfo and pre-state init values. Must run after
    // the AST + IR scans so the explicit `init_value: Some("0")` for the
    // error flag isn't shadowed by a duplicate entry from
    // `discover_ir_only_globals` (which would have `init_value: None`
    // because it can't parse the LLVM `false` literal).
    if let Some(ir_path) = llvm_ir_path {
        crate::transform::eh_globals::inject_exclow_globals(&mut all_globals, ir_path);
    }
    Ok((all_globals, ir_struct_defs))
}

/// Finalize aliases without flattening exact names from the validated object plan.
#[allow(clippy::too_many_arguments)]
pub fn finalize_aliases(
    parsed_ast: &clang_ast::AstNode,
    all_functions: &[FunctionInfo],
    ir_funcs: &[FunctionInfo],
    ir_struct_sizes: &HashMap<String, usize>,
    alias_size_overrides: &[String],
    alias_enum_overrides: &[String],
    output: &Path,
    protected: &HashSet<String>,
) -> Result<()> {
    let mut fallbacks = collect_type_sizes(all_functions);
    // Seed enum_bits from every EnumDecl in the AST so forward-declared
    // enums like `LatchResult` still get the `llvm_int <bits>` fallback.
    for (name, bits) in clang_ast::collect_all_enum_bits(parsed_ast) {
        fallbacks.enum_bits.entry(name).or_insert(bits);
    }
    if !ir_funcs.is_empty() {
        crate::alias_fallbacks_ir::add_ir_deref_fallbacks(&mut fallbacks, all_functions, ir_funcs);
    }
    // CLI overrides take priority over inferred sizes.
    apply_cli_overrides(&mut fallbacks, alias_size_overrides, alias_enum_overrides)?;
    // SAW_SPEC_GEN_DEBUG_FALLBACKS=1 to see resolved fallback sizes.
    if std::env::var_os("SAW_SPEC_GEN_DEBUG_FALLBACKS").is_some() {
        dump_fallback_diagnostics(&fallbacks);
    }
    apply_alias_rewrites_protected(output, ir_struct_sizes, &fallbacks, protected);
    Ok(())
}
