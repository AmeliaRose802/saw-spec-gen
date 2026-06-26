//! Native `saw-spec-gen verify-cpp` implementation for C++ targets.

mod compile;
mod counterexample;

use crate::inventory;
use crate::verify_cache::AstCacheContext;
use crate::verify_result::write_verify_result;
use crate::verify_tools::ToolPaths;
use anyhow::{bail, Context, Result};
use compile::{
    build_clang_flags, dump_ast, emit_bitcode, emit_llvm_ir, is_spec_only_result, maybe_filter_ast,
    maybe_lower_exceptions, patch_ir_and_reassemble, recompile_at_o1, run_gen_verify, O1Recompile,
};
use counterexample::{evaluate_counterexample, parse_counterexample, report_disproved};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub struct VerifyOutcome {
    pub exit_code: i32,
}

pub struct VerifyRequest {
    pub cpp_file: PathBuf,
    pub cryptol_spec: PathBuf,
    pub cryptol_fn: String,
    pub function: String,
    pub output: Option<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub cxx_standard: Option<String>,
    pub clang_flags: Vec<String>,
    pub extra_spec_gen_args: Vec<String>,
    pub spec_only_on_missing: bool,
}

pub fn run(req: VerifyRequest) -> Result<VerifyOutcome> {
    let cpp_file = req
        .cpp_file
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", req.cpp_file.display()))?;
    let cryptol_spec = req
        .cryptol_spec
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", req.cryptol_spec.display()))?;
    let include_dirs = req
        .include_dirs
        .iter()
        .map(|p| {
            p.canonicalize()
                .with_context(|| format!("failed to resolve include dir {}", p.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let base_name = cpp_file
        .file_stem()
        .and_then(OsStr::to_str)
        .context("cpp file has no basename")?
        .to_string();
    let mut output_dir = req.output.unwrap_or_else(|| {
        cpp_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("out_{base_name}"))
    });
    if output_dir.exists() {
        std::fs::remove_dir_all(&output_dir)?;
    }
    std::fs::create_dir_all(&output_dir)?;
    output_dir = output_dir.canonicalize()?;

    let tools = ToolPaths::discover();
    let clang = tools
        .clang
        .as_deref()
        .context("could not find clang (set SAW_SPEC_GEN_LLVM_BIN or add clang to PATH)")?;
    let llvm_as = tools
        .llvm_as
        .as_deref()
        .context("could not find llvm-as (set SAW_SPEC_GEN_LLVM_BIN or add llvm-as to PATH)")?;
    let saw = tools
        .saw
        .as_deref()
        .context("could not find saw (set SAW_SPEC_GEN_SAW or add saw to PATH)")?;
    tools.add_discovered_dirs_to_path();

    let is_msvc = tools.llvm_target.contains("windows-msvc");
    let user_clang_flags =
        build_clang_flags(&include_dirs, req.cxx_standard.as_deref(), &req.clang_flags);
    let bc_file = output_dir.join(format!("{base_name}.bc"));
    let ll_file = output_dir.join(format!("{base_name}.ll"));
    let ast_file = output_dir.join(format!("{base_name}_ast.json"));

    let cache = AstCacheContext::load(
        &cpp_file,
        &include_dirs,
        &user_clang_flags,
        tools.llvm_target,
        &output_dir,
        &base_name,
        &bc_file,
        &ll_file,
        &ast_file,
    )?;
    let mut ll_for_gen = cache.ll_file.clone();
    if !cache.hit {
        emit_bitcode(
            clang,
            tools.llvm_target,
            &user_clang_flags,
            &cpp_file,
            &bc_file,
            "-O0",
        )?;
        ll_for_gen = emit_llvm_ir(
            clang,
            tools.llvm_target,
            &user_clang_flags,
            &cpp_file,
            &ll_file,
            "-O0",
        )?;
        maybe_lower_exceptions(
            &tools,
            is_msvc,
            &output_dir,
            &base_name,
            &bc_file,
            ll_for_gen.as_deref(),
        )?;
        if ll_for_gen.is_some() {
            patch_ir_and_reassemble(llvm_as, &ll_file, &bc_file)?;
        }
        dump_ast(
            clang,
            tools.llvm_target,
            &user_clang_flags,
            &cpp_file,
            &ast_file,
        )?;
        maybe_filter_ast(&cpp_file, &ast_file)?;
        cache.save(&bc_file, ll_for_gen.as_deref(), &ast_file)?;
    }

    let cry_dest = output_dir.join(
        cryptol_spec
            .file_name()
            .context("cryptol spec file has no basename")?,
    );
    std::fs::copy(&cryptol_spec, &cry_dest)?;
    run_gen_verify(
        &output_dir,
        &bc_file,
        ll_for_gen.as_deref(),
        &ast_file,
        &cry_dest,
        &req.cryptol_fn,
        &req.function,
        &req.extra_spec_gen_args,
        req.spec_only_on_missing,
    )?;
    if req.spec_only_on_missing && is_spec_only_result(&output_dir)? {
        eprintln!(
            "spec-only: no C++ implementation for '{}' — skipping SAW.",
            req.function
        );
        return Ok(VerifyOutcome { exit_code: 0 });
    }

    let saw_started = Instant::now();
    let mut saw_output = run_saw(saw, &output_dir, "verify.saw")?;
    // Part 2 (§2): one-shot `-O0` → `-O1` fallback. The `-O0` STL
    // build can emit empty-struct global loads (e.g. from
    // `std::optional` / `std::nullopt_t` ctors) that SAW's simulator
    // cannot load; `-O1` inlines them into plain byte stores. Only
    // fires on a non-conclusive result, so a real VERIFIED/DISPROVED
    // verdict is never overridden.
    if !saw_output.contains("VERIFIED")
        && !saw_output.contains("Counterexample")
        && is_empty_struct_load_failure(&saw_output)
    {
        eprintln!(
            "note: SAW could not load the -O0 build (empty-struct global load); \
             retrying once at -O1."
        );
        recompile_at_o1(&O1Recompile {
            tools: &tools,
            is_msvc,
            clang,
            llvm_as,
            output_dir: &output_dir,
            base_name: &base_name,
            user_flags: &user_clang_flags,
            cpp_file: &cpp_file,
            bc_file: &bc_file,
            ll_file: &ll_file,
            ast_file: &ast_file,
            cry_dest: &cry_dest,
            cryptol_fn: &req.cryptol_fn,
            function: &req.function,
            extra_spec_gen_args: &req.extra_spec_gen_args,
            spec_only_on_missing: req.spec_only_on_missing,
        })?;
        saw_output = run_saw(saw, &output_dir, "verify.saw")?;
    }
    let time_secs = saw_started.elapsed().as_secs_f64();
    let impl_file = cpp_file.file_name().and_then(OsStr::to_str);
    if saw_output.contains("Counterexample") {
        let counterexample = parse_counterexample(&saw_output);
        let (expected, actual) = evaluate_counterexample(
            saw,
            clang,
            &cpp_file,
            &cry_dest,
            &output_dir,
            tools.llvm_target,
            tools.exe_ext,
            &user_clang_flags,
            &req.cryptol_fn,
            &req.function,
            &counterexample,
        )?;
        report_disproved(
            &tools,
            is_msvc,
            &req.function,
            &req.cryptol_fn,
            &saw_output,
            &counterexample,
        );
        write_verify_result(
            &output_dir,
            "cpp",
            &req.function,
            &req.cryptol_fn,
            "DISPROVED",
            &counterexample,
            expected.as_deref(),
            actual.as_deref(),
            Some("z3"),
            Some(time_secs),
            impl_file,
        )?;
        update_inventory(&output_dir)?;
        return Ok(VerifyOutcome { exit_code: 1 });
    }
    if saw_output.contains("VERIFIED") {
        println!("RESULT: VERIFIED");
        write_verify_result(
            &output_dir,
            "cpp",
            &req.function,
            &req.cryptol_fn,
            "VERIFIED",
            &[],
            None,
            None,
            Some("z3"),
            Some(time_secs),
            impl_file,
        )?;
        update_inventory(&output_dir)?;
        return Ok(VerifyOutcome { exit_code: 0 });
    }
    println!("RESULT: UNKNOWN");
    write_verify_result(
        &output_dir,
        "cpp",
        &req.function,
        &req.cryptol_fn,
        "UNKNOWN",
        &[],
        None,
        None,
        Some("z3"),
        Some(time_secs),
        impl_file,
    )?;
    update_inventory(&output_dir)?;
    Ok(VerifyOutcome { exit_code: 2 })
}

fn run_saw(saw: &Path, output_dir: &Path, script_name: &str) -> Result<String> {
    let out = Command::new(saw)
        .arg(script_name)
        .current_dir(output_dir)
        .output()?;
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    print!("{text}");
    Ok(text)
}

/// Heuristic: does this SAW transcript look like the `-O0` empty-struct
/// global-load failure that a `-O1` rebuild fixes? Used only to gate the
/// one-shot recompile fallback, and only on an otherwise non-conclusive
/// run, so a false positive merely costs one extra `-O1` attempt.
fn is_empty_struct_load_failure(saw_output: &str) -> bool {
    saw_output.contains("Error during memory load")
        || saw_output.contains("Cannot load through pointer")
        || saw_output.contains("Unexpected zero-sized")
}

fn update_inventory(output_dir: &Path) -> Result<()> {
    let root = output_dir.parent().unwrap_or(output_dir);
    inventory::aggregate_inventory(root, &root.join("implementation_inventory.json"))
}

fn run_command(cmd: &mut Command, label: &str) -> Result<()> {
    let out = cmd.output()?;
    if !out.status.success() {
        bail!(
            "{label} failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_empty_struct_load_failure;

    #[test]
    fn detects_memory_load_failure_signatures() {
        assert!(is_empty_struct_load_failure(
            "saw: Error during memory load"
        ));
        assert!(is_empty_struct_load_failure(
            "Cannot load through pointer to empty struct"
        ));
        assert!(is_empty_struct_load_failure("Unexpected zero-sized type"));
    }

    #[test]
    fn ignores_conclusive_or_unrelated_output() {
        assert!(!is_empty_struct_load_failure("RESULT: VERIFIED"));
        assert!(!is_empty_struct_load_failure("Counterexample: x = 3"));
        assert!(!is_empty_struct_load_failure("z3: unknown"));
    }
}
