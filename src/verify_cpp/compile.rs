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
) -> Result<()> {
    run_command(
        Command::new(clang)
            .args([
                "-c",
                "-emit-llvm",
                "-O0",
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
) -> Result<Option<PathBuf>> {
    let out = Command::new(clang)
        .args([
            "-S",
            "-emit-llvm",
            "-O0",
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
    extra_spec_gen_args: &[String],
    spec_only_on_missing: bool,
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
    if spec_only_on_missing {
        cmd.arg("--spec-only-on-missing");
    }
    cmd.args(extra_spec_gen_args);
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
