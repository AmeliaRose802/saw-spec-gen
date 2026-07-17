//! Clang/LLVM compilation pipeline helpers for the native C++ verify flow.

use super::run_command;
use crate::verify_tools::ToolPaths;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Strip the Windows extended-length / verbatim prefix (`\\?\` or `\\?\UNC\`)
/// from a path string. clang resolves quoted `#include "a/b.h"` directives
/// relative to the including file's directory and the `-I` search dirs by
/// textual path joining; when that base path carries a `\\?\` verbatim prefix
/// (as produced by `Path::canonicalize` on Windows) the join keeps the `..`
/// components literal — verbatim paths are never normalized — so the header is
/// reported "file not found". Passing the plain absolute path lets the Win32
/// path layer collapse `..` and keeps both clang and the OS happy.
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Render a path as a clang command-line argument with any Windows verbatim
/// prefix stripped. Used for the source file so clang can resolve relative
/// `#include "../.."` directives against the on-disk directory.
fn clang_path_arg(path: &Path) -> String {
    strip_verbatim_prefix(&path.display().to_string())
}

pub(super) fn build_clang_flags(
    include_dirs: &[PathBuf],
    cxx_standard: Option<&str>,
    clang_flags: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for dir in include_dirs {
        out.push("-I".to_string());
        out.push(strip_verbatim_prefix(&dir.display().to_string()));
    }
    if let Some(std) = cxx_standard {
        out.push(format!("-std={std}"));
    }
    out.extend(clang_flags.iter().cloned());
    out
}

pub(super) fn emit_bitcode(
    clang: &Path,
    llvm_target: &str,
    user_flags: &[String],
    cpp_file: &Path,
    bc_file: &Path,
    opt_level: &str,
) -> Result<()> {
    run_command(
        Command::new(clang)
            .args([
                "-c",
                "-emit-llvm",
                opt_level,
                "-fno-rtti",
                "-target",
                llvm_target,
            ])
            .args(user_flags)
            .arg(clang_path_arg(cpp_file))
            .arg("-o")
            .arg(bc_file),
        "clang bitcode",
    )
}

pub(super) fn emit_llvm_ir(
    clang: &Path,
    llvm_target: &str,
    user_flags: &[String],
    cpp_file: &Path,
    ll_file: &Path,
    opt_level: &str,
) -> Result<Option<PathBuf>> {
    let out = Command::new(clang)
        .args([
            "-S",
            "-emit-llvm",
            opt_level,
            "-fno-rtti",
            "-target",
            llvm_target,
        ])
        .args(user_flags)
        .arg(clang_path_arg(cpp_file))
        .arg("-o")
        .arg(ll_file)
        .output()?;
    if out.status.success() {
        Ok(Some(ll_file.to_path_buf()))
    } else {
        eprintln!("warning: failed to emit .ll; continuing without --llvm-ir");
        Ok(None)
    }
}

pub(super) fn maybe_lower_exceptions(
    tools: &ToolPaths,
    is_msvc: bool,
    output_dir: &Path,
    base_name: &str,
    bc_file: &Path,
    ll_file: Option<&Path>,
) -> Result<()> {
    let Some(exception_lower) = tools.exception_lower.as_deref() else {
        if is_msvc {
            eprintln!("note: exception-lower binary not available; EH will not be lowered.");
        }
        return Ok(());
    };
    let lowered_bc = output_dir.join(format!("{base_name}_lowered.bc"));
    let out = Command::new(exception_lower)
        .arg(bc_file)
        .arg("-o")
        .arg(&lowered_bc)
        .output()?;
    if !out.status.success() {
        eprintln!("warning: exception-lower failed; continuing with unlowered IR");
        return Ok(());
    }
    std::fs::copy(&lowered_bc, bc_file)?;
    if let (Some(ll_file), Some(llvm_dis)) = (ll_file, tools.llvm_dis.as_deref()) {
        run_command(
            Command::new(llvm_dis).arg(bc_file).arg("-o").arg(ll_file),
            "llvm-dis after exception-lower",
        )?;
    }
    Ok(())
}

pub(super) fn patch_ir_and_reassemble(
    llvm_as: &Path,
    ll_file: &Path,
    bc_file: &Path,
) -> Result<()> {
    crate::patch_llvm_ir::patch_llvm_ir_file(ll_file, ll_file, false)?;
    run_command(
        Command::new(llvm_as).arg(ll_file).arg("-o").arg(bc_file),
        "llvm-as post-patch",
    )
}

pub(super) fn dump_ast(
    clang: &Path,
    llvm_target: &str,
    user_flags: &[String],
    cpp_file: &Path,
    ast_file: &Path,
) -> Result<()> {
    let out = Command::new(clang)
        .args([
            "-Xclang",
            "-ast-dump=json",
            "-fsyntax-only",
            "-target",
            llvm_target,
        ])
        .args(user_flags)
        .arg(clang_path_arg(cpp_file))
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("clang AST dump failed");
    }
    std::fs::write(ast_file, out.stdout)?;
    Ok(())
}

pub(super) fn maybe_filter_ast(cpp_file: &Path, ast_file: &Path) -> Result<()> {
    let size_mb = std::fs::metadata(ast_file)?.len() / (1024 * 1024);
    if size_mb <= 10 {
        return Ok(());
    }
    let keep = vec![cpp_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()];
    crate::clang_ast::filter_ast_file(ast_file, ast_file, &keep)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_gen_verify(
    output_dir: &Path,
    bc_file: &Path,
    ll_file: Option<&Path>,
    ast_file: &Path,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
    config: Option<&Path>,
) -> Result<()> {
    let self_exe = std::env::current_exe()?;
    let mut cmd = Command::new(self_exe);
    cmd.arg("gen-verify")
        .arg("--ast")
        .arg(ast_file)
        .arg("--bitcode")
        .arg(bc_file)
        .arg("--cryptol-spec")
        .arg(cryptol_spec)
        .arg("--cryptol-fn")
        .arg(cryptol_fn)
        .arg("--function")
        .arg(function)
        .arg("--output")
        .arg(output_dir);
    if let Some(ll_file) = ll_file {
        cmd.arg("--llvm-ir").arg(ll_file);
    }
    if let Some(config) = config {
        cmd.arg("--config").arg(config);
    }
    run_command(&mut cmd, "gen-verify")
}

pub(super) fn is_spec_only_result(output_dir: &Path) -> Result<bool> {
    let result_path = output_dir.join("result.json");
    if !result_path.exists() {
        return Ok(false);
    }
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(result_path)?)?;
    Ok(value.get("status").and_then(|v| v.as_str()) == Some("not_attempted"))
}

/// Inputs for the `optnone`-strip + `opt` promotion fallback
/// ([`recompile_promoted`]).
pub(super) struct PromoteRecompile<'a> {
    pub tools: &'a ToolPaths,
    pub llvm_as: &'a Path,
    pub output_dir: &'a Path,
    pub base_name: &'a str,
    pub bc_file: &'a Path,
    pub ll_file: &'a Path,
    pub ast_file: &'a Path,
    pub cryptol_spec: &'a Path,
    pub cryptol_fn: &'a str,
    pub function: &'a str,
    pub config: Option<&'a Path>,
}

/// Promote the existing `-O0` IR *without inlining*: strip the `optnone`
/// function attribute so `opt` will touch the STL bodies, then run
/// `sroa,mem2reg,instsimplify`. This rewrites the uninitialized reads of
/// stateless empty-tag / functor allocas (`std::equal_to` inside
/// `std::equal`, `std::nullopt_t` / `std::in_place_t` inside the
/// `std::optional` copy/move ctors) into `undef` register values —
/// eliminating the vacuous `Error during memory load` — while keeping
/// every function un-inlined so the mutex / `memcmp` overrides emitted in
/// the generated spec still apply. (The `-O1` fallback, by contrast,
/// inlines the MSVC mutex ownership-count check into an `unreachable`.)
///
/// Returns `Ok(false)` when `opt` is unavailable (the caller should fall
/// through to the `-O1` recompile); `Ok(true)` when the promoted build
/// and verify script were regenerated.
pub(super) fn recompile_promoted(ctx: &PromoteRecompile) -> Result<bool> {
    let Some(opt) = ctx.tools.opt.as_deref() else {
        eprintln!("note: `opt` not found; cannot promote -O0 IR, falling back to -O1.");
        return Ok(false);
    };
    let ll_text = std::fs::read_to_string(ctx.ll_file)?;
    let stripped = strip_optnone(&ll_text);
    let promoted_ll = ctx
        .output_dir
        .join(format!("{}_promoted.ll", ctx.base_name));
    std::fs::write(&promoted_ll, stripped)?;
    let noopt_bc = ctx.output_dir.join(format!("{}_noopt.bc", ctx.base_name));
    run_command(
        Command::new(ctx.llvm_as)
            .arg(&promoted_ll)
            .arg("-o")
            .arg(&noopt_bc),
        "llvm-as (optnone-stripped)",
    )?;
    let promoted_bc = ctx
        .output_dir
        .join(format!("{}_promoted.bc", ctx.base_name));
    run_command(
        Command::new(opt)
            .arg("-passes=sroa,mem2reg,instsimplify")
            .arg(&noopt_bc)
            .arg("-o")
            .arg(&promoted_bc),
        "opt promote (sroa,mem2reg,instsimplify)",
    )?;
    std::fs::copy(&promoted_bc, ctx.bc_file)?;
    // Reuse the original `-O0` `.ll` for `--llvm-ir`: stripping `optnone`
    // and running sroa/mem2reg/instsimplify never changes a function's
    // ABI signature, so the sret / param-attribute analysis is identical.
    run_gen_verify(
        ctx.output_dir,
        ctx.bc_file,
        Some(ctx.ll_file),
        ctx.ast_file,
        ctx.cryptol_spec,
        ctx.cryptol_fn,
        ctx.function,
        ctx.config,
    )?;
    Ok(true)
}

/// Remove the `optnone` function attribute from every `attributes #N = {`
/// group line of an LLVM IR module. `optnone` (emitted by clang at `-O0`)
/// makes `opt` skip the function entirely, so stripping it is a
/// prerequisite for the sroa/mem2reg promotion. Restricting the edit to
/// `attributes` group lines avoids disturbing any identically named
/// global or metadata elsewhere in the module.
fn strip_optnone(ll: &str) -> String {
    let mut out = String::with_capacity(ll.len());
    for line in ll.lines() {
        if line.trim_start().starts_with("attributes #") {
            out.push_str(&line.replace(" optnone", ""));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Inputs for the one-shot `-O1` recompile fallback ([`recompile_at_o1`]).
pub(super) struct O1Recompile<'a> {
    pub tools: &'a ToolPaths,
    pub is_msvc: bool,
    pub clang: &'a Path,
    pub llvm_as: &'a Path,
    pub output_dir: &'a Path,
    pub base_name: &'a str,
    pub user_flags: &'a [String],
    pub cpp_file: &'a Path,
    pub bc_file: &'a Path,
    pub ll_file: &'a Path,
    pub ast_file: &'a Path,
    pub cryptol_spec: &'a Path,
    pub cryptol_fn: &'a str,
    pub function: &'a str,
    pub config: Option<&'a Path>,
}

/// Recompile the C++ target at `-O1` and regenerate the verify script.
///
/// Used as a single defensive fallback when the `-O0` build produced
/// SAW IR that the simulator cannot load — typically the empty-struct
/// global loads emitted by `std::optional` / `std::nullopt_t` /
/// `std::in_place_t` constructors that `-O1` inlines into plain byte
/// stores. The clang AST is opt-level-independent, so it is reused.
pub(super) fn recompile_at_o1(ctx: &O1Recompile) -> Result<()> {
    let target = ctx.tools.llvm_target;
    emit_bitcode(
        ctx.clang,
        target,
        ctx.user_flags,
        ctx.cpp_file,
        ctx.bc_file,
        "-O1",
    )?;
    let ll = emit_llvm_ir(
        ctx.clang,
        target,
        ctx.user_flags,
        ctx.cpp_file,
        ctx.ll_file,
        "-O1",
    )?;
    maybe_lower_exceptions(
        ctx.tools,
        ctx.is_msvc,
        ctx.output_dir,
        ctx.base_name,
        ctx.bc_file,
        ll.as_deref(),
    )?;
    if ll.is_some() {
        patch_ir_and_reassemble(ctx.llvm_as, ctx.ll_file, ctx.bc_file)?;
    }
    run_gen_verify(
        ctx.output_dir,
        ctx.bc_file,
        ll.as_deref(),
        ctx.ast_file,
        ctx.cryptol_spec,
        ctx.cryptol_fn,
        ctx.function,
        ctx.config,
    )
}

#[cfg(test)]
mod tests {
    use super::strip_optnone;

    #[test]
    fn strips_optnone_only_from_attribute_groups() {
        let ll = "define void @f() #0 {\n  ret void\n}\n\
                  attributes #0 = { noinline optnone uwtable }\n";
        let out = strip_optnone(ll);
        // The attributes group loses `optnone`...
        assert!(out.contains("attributes #0 = { noinline uwtable }"));
        // ...but the function body and its `#0` reference are untouched.
        assert!(out.contains("define void @f() #0 {"));
        assert!(!out.contains("optnone"));
    }

    #[test]
    fn leaves_non_attribute_lines_intact() {
        // A stray "optnone" substring outside an attributes group (e.g.
        // in a symbol or comment) must NOT be stripped.
        let ll = "@optnone_global = global i8 0\n\
                  attributes #1 = { optnone }\n";
        let out = strip_optnone(ll);
        assert!(out.contains("@optnone_global = global i8 0"));
        assert!(out.contains("attributes #1 = { }"));
    }
}
