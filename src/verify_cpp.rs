//! Native `saw-spec-gen verify` implementation for C++ targets.

use crate::inventory;
use crate::verify_cache::AstCacheContext;
use crate::verify_result::{write_verify_result, CounterexampleEntry};
use crate::verify_tools::ToolPaths;
use anyhow::{bail, Context, Result};
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
        )?;
        ll_for_gen = emit_llvm_ir(
            clang,
            tools.llvm_target,
            &user_clang_flags,
            &cpp_file,
            &ll_file,
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
    let saw_output = run_saw(saw, &output_dir, "verify.saw")?;
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

fn build_clang_flags(
    include_dirs: &[PathBuf],
    cxx_standard: Option<&str>,
    clang_flags: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for dir in include_dirs {
        out.push("-I".to_string());
        out.push(dir.display().to_string());
    }
    if let Some(std) = cxx_standard {
        out.push(format!("-std={std}"));
    }
    out.extend(clang_flags.iter().cloned());
    out
}

fn emit_bitcode(
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
            .arg(cpp_file)
            .arg("-o")
            .arg(bc_file),
        "clang bitcode",
    )
}

fn emit_llvm_ir(
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
        .arg(cpp_file)
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

fn maybe_lower_exceptions(
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

fn patch_ir_and_reassemble(llvm_as: &Path, ll_file: &Path, bc_file: &Path) -> Result<()> {
    crate::patch_llvm_ir::patch_llvm_ir_file(ll_file, ll_file, false)?;
    run_command(
        Command::new(llvm_as).arg(ll_file).arg("-o").arg(bc_file),
        "llvm-as post-patch",
    )
}

fn dump_ast(
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
        .arg(cpp_file)
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("clang AST dump failed");
    }
    std::fs::write(ast_file, out.stdout)?;
    Ok(())
}

fn maybe_filter_ast(cpp_file: &Path, ast_file: &Path) -> Result<()> {
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
fn run_gen_verify(
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

fn is_spec_only_result(output_dir: &Path) -> Result<bool> {
    let result_path = output_dir.join("result.json");
    if !result_path.exists() {
        return Ok(false);
    }
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(result_path)?)?;
    Ok(value.get("status").and_then(|v| v.as_str()) == Some("not_attempted"))
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

fn parse_counterexample(saw_output: &str) -> Vec<CounterexampleEntry> {
    saw_output
        .lines()
        .filter_map(parse_counterexample_line)
        .map(|(name, value)| CounterexampleEntry {
            name,
            value: Some(value),
            bits: None,
        })
        .collect()
}

fn parse_counterexample_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let (name, value) = trimmed.split_once(':')?;
    let value = value.trim();
    if name.is_empty() || value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((name.trim().to_string(), value.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_counterexample(
    saw: &Path,
    clang: &Path,
    cpp_file: &Path,
    cry_dest: &Path,
    output_dir: &Path,
    llvm_target: &str,
    exe_ext: &str,
    user_clang_flags: &[String],
    cryptol_fn: &str,
    function: &str,
    counterexample: &[CounterexampleEntry],
) -> Result<(Option<String>, Option<String>)> {
    if counterexample.is_empty() {
        return Ok((None, None));
    }
    let cryptol_args = counterexample
        .iter()
        .filter_map(|c| c.value.as_deref())
        .map(|v| format!("({v} : [32])"))
        .collect::<Vec<_>>()
        .join(" ");
    let eval_script = output_dir.join("_eval_cex.saw");
    let cry_name = cry_dest
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("spec.cry");
    std::fs::write(
        &eval_script,
        format!(
            "import \"{cry_name}\";\nlet r = eval_int {{ {cryptol_fn} {cryptol_args} }};\nprint (str_concat \"CRYPTOL_RESULT=\" (show r));\n"
        ),
    )?;
    let eval_out = run_saw(saw, output_dir, "_eval_cex.saw")?;
    let expected = capture_suffix(&eval_out, "CRYPTOL_RESULT=");

    let test_cpp = output_dir.join("_test_cex.cpp");
    let test_exe = output_dir.join(format!("_test_cex{exe_ext}"));
    let cpp_args = counterexample
        .iter()
        .filter_map(|c| c.value.as_deref())
        .map(|v| format!("{v}u"))
        .collect::<Vec<_>>()
        .join(", ");
    let orig_src = std::fs::read_to_string(cpp_file)?;
    std::fs::write(
        &test_cpp,
        format!(
            "{orig_src}\n\n#include <cstdio>\n#include <cstring>\nint main() {{\n  auto result = {function}({cpp_args});\n  unsigned long long bits = 0;\n  size_t n = sizeof(result) < sizeof(bits) ? sizeof(result) : sizeof(bits);\n  std::memcpy(&bits, &result, n);\n  std::printf(\"CPP_RESULT=%llu\\n\", bits);\n  return 0;\n}}\n"
        ),
    )?;
    let compile = Command::new(clang)
        .args(["-O0", "-target", llvm_target])
        .args(user_clang_flags)
        .arg(&test_cpp)
        .arg("-o")
        .arg(&test_exe)
        .output()?;
    let actual = if compile.status.success() && test_exe.exists() {
        let out = Command::new(&test_exe).output()?;
        capture_suffix(&String::from_utf8_lossy(&out.stdout), "CPP_RESULT=")
    } else {
        None
    };
    Ok((expected, actual))
}

fn capture_suffix(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(prefix)
            .map(str::trim)
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
            .map(ToString::to_string)
    })
}

fn report_disproved(
    tools: &ToolPaths,
    is_msvc: bool,
    function: &str,
    cryptol_fn: &str,
    saw_output: &str,
    counterexample: &[CounterexampleEntry],
) {
    println!("RESULT: DISPROVED");
    eprintln!("C++ function  : {function}");
    eprintln!("Cryptol spec  : {cryptol_fn}");
    if !counterexample.is_empty() {
        eprintln!("Counterexample:");
        for entry in counterexample {
            if let Some(value) = entry.value.as_deref() {
                eprintln!("  {} = {}", entry.name, value);
            }
        }
    }
    if let Some(sym) = saw_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Subgoal failed:").map(str::trim))
    {
        eprintln!("Failed proof obligation: {}", demangle(tools, is_msvc, sym));
    }
}

fn demangle(tools: &ToolPaths, is_msvc: bool, mangled: &str) -> String {
    let Some(cmd) = tools.cxxfilt.as_deref() else {
        return mangled.to_string();
    };
    let Ok(out) = Command::new(cmd).arg(mangled).output() else {
        return mangled.to_string();
    };
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if is_msvc {
        if let Some(body) = raw.split("is :- ").nth(1) {
            return body
                .trim_matches('"')
                .replace("__cdecl", "")
                .trim()
                .to_string();
        }
        return mangled.to_string();
    }
    if raw.is_empty() || raw == mangled {
        mangled.to_string()
    } else {
        raw
    }
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
