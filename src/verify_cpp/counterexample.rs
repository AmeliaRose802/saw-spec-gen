//! Counterexample parsing, replay, and disproved-result reporting.

use super::run_saw;
use crate::verify_result::CounterexampleEntry;
use crate::verify_tools::ToolPaths;
use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

pub(super) fn parse_counterexample(saw_output: &str) -> Vec<CounterexampleEntry> {
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
pub(super) fn evaluate_counterexample(
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
            "import \"{cry_name}\";\nlet r = eval_int {{{{ {cryptol_fn} {cryptol_args} }}}};\nprint (str_concat \"CRYPTOL_RESULT=\" (show r));\n"
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

pub(super) fn report_disproved(
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
