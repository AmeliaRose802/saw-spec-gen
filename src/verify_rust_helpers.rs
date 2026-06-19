use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
pub(crate) struct RustMetaParam {
    pub(crate) bits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CexBinding {
    pub(crate) index: usize,
    pub(crate) name: String,
    pub(crate) value: u64,
    pub(crate) bits: u32,
}

pub(crate) fn run_and_print(cmd: &mut Command) -> Result<()> {
    let text = run_capture(cmd)?;
    print!("{text}");
    Ok(())
}

pub(crate) fn run_capture(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("failed to run command: {:?}", cmd))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!(
            "command failed (exit={}): {:?}\n{}",
            out.status.code().unwrap_or(-1),
            cmd,
            text
        );
    }
    Ok(text)
}

/// Run a command and always return captured stdout/stderr text.
///
/// Unlike `run_capture`, this does not treat non-zero exit codes as an
/// error, which is required when downstream logic classifies tool output.
pub(crate) fn run_capture_allow_failure(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("failed to run command: {:?}", cmd))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(text)
}

pub(crate) fn parse_counterexample_bindings(
    saw_out: &str,
    params: &[RustMetaParam],
) -> Vec<CexBinding> {
    let mut out = vec![];
    for line in saw_out.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('x') {
            continue;
        }
        let Some((lhs, rhs)) = trimmed.split_once(':') else {
            continue;
        };
        if !lhs.starts_with('x') {
            continue;
        }
        let idx = lhs[1..].trim().parse::<usize>();
        let value = rhs.trim().parse::<u64>();
        let (Ok(index), Ok(value)) = (idx, value) else {
            continue;
        };
        let bits = params.get(index).map(|p| p.bits).unwrap_or(32);
        out.push(CexBinding {
            index,
            name: lhs.to_string(),
            value,
            bits,
        });
    }
    out.sort_by_key(|x| x.index);
    out
}

pub(crate) fn eval_cryptol_at_cex(
    saw: &Path,
    output_dir: &Path,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    cex: &[CexBinding],
) -> Result<Option<String>> {
    if cex.is_empty() {
        return Ok(None);
    }
    let cry_name = cryptol_spec
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid cryptol spec file name"))?;
    let cryptol_args = cex
        .iter()
        .map(|c| {
            if c.bits == 1 {
                format!("(({} : [1]) ! 0)", c.value)
            } else {
                format!("({} : [{}])", c.value, c.bits)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let eval_script = output_dir.join("_eval_cex.saw");
    fs::write(
        &eval_script,
        format!(
            "import \"{cry_name}\";\nlet r = eval_int {{ {cryptol_fn} {cryptol_args} }};\nprint (str_concat \"CRYPTOL_RESULT=\" (show r));\n"
        ),
    )?;
    let out = run_capture(
        Command::new(saw)
            .arg("_eval_cex.saw")
            .current_dir(output_dir),
    )?;
    for line in out.lines() {
        if let Some(v) = line.trim().strip_prefix("CRYPTOL_RESULT=") {
            return Ok(Some(v.trim().to_string()));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_rust_harness_at_cex(
    rustc: &Path,
    llvm_target: &str,
    exe_ext: &str,
    output_dir: &Path,
    rust_file: &Path,
    function: &str,
    rust_param_types: &[String],
    cex: &[CexBinding],
) -> Result<Option<String>> {
    if cex.is_empty() || cex.len() != rust_param_types.len() {
        return Ok(None);
    }
    let mut call_args = Vec::with_capacity(cex.len());
    for (i, c) in cex.iter().enumerate() {
        let rust_type = &rust_param_types[i];
        if rust_type == "bool" {
            call_args.push(if c.value == 0 { "false" } else { "true" }.to_string());
        } else {
            call_args.push(format!("({}u64 as {})", c.value, rust_type));
        }
    }
    let harness = output_dir.join("_harness.rs");
    let harness_exe = output_dir.join(format!("_harness{exe_ext}"));
    let rust_file_for_include = rust_file.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &harness,
        format!(
            "// Auto-generated: calls {function} at SAW counterexample inputs.\n\
             include!(r\"{rust_file_for_include}\");\n\
             #[allow(dead_code)]\n\
             fn main() {{\n\
                 let r = {function}({});\n\
                 let mut bits: u64 = 0;\n\
                 let n = std::cmp::min(std::mem::size_of_val(&r), std::mem::size_of::<u64>());\n\
                 unsafe {{ std::ptr::copy_nonoverlapping(&r as *const _ as *const u8, &mut bits as *mut _ as *mut u8, n); }}\n\
                 println!(\"RUST_RESULT={{}}\", bits);\n\
             }}\n",
            call_args.join(", ")
        ),
    )?;

    let build = run_capture(
        Command::new(rustc)
            .arg("--crate-type=bin")
            .arg("--edition=2021")
            .arg("--target")
            .arg(llvm_target)
            .arg("-C")
            .arg("opt-level=0")
            .arg("-C")
            .arg("overflow-checks=off")
            .arg("-C")
            .arg("debug-assertions=off")
            .arg("-C")
            .arg("panic=abort")
            .arg("-C")
            .arg("codegen-units=1")
            .arg("-C")
            .arg("debuginfo=0")
            .arg("-A")
            .arg("dead_code")
            .arg("-A")
            .arg("unused")
            .arg("-o")
            .arg(&harness_exe)
            .arg(&harness),
    );

    if let Err(err) = build {
        println!("  Rust counterexample harness failed to compile:");
        println!("  {err}");
        return Ok(None);
    }

    if !harness_exe.exists() {
        return Ok(None);
    }
    let out = run_capture(&mut Command::new(&harness_exe))?;
    for line in out.lines() {
        if let Some(v) = line.trim().strip_prefix("RUST_RESULT=") {
            return Ok(Some(v.trim().to_string()));
        }
    }
    Ok(None)
}

pub(crate) fn parse_rust_signature(src: &str, function: &str) -> (Vec<String>, Vec<String>) {
    let needle = format!("fn {function}");
    let Some(fn_pos) = src.find(&needle) else {
        return (vec![], vec![]);
    };
    let rest = &src[fn_pos + needle.len()..];
    let Some(open_rel) = rest.find('(') else {
        return (vec![], vec![]);
    };
    let after_open = &rest[open_rel + 1..];
    let Some(close_rel) = after_open.find(')') else {
        return (vec![], vec![]);
    };
    let param_list = after_open[..close_rel].trim();
    if param_list.is_empty() {
        return (vec![], vec![]);
    }
    let mut names = vec![];
    let mut types = vec![];
    for piece in param_list.split(',') {
        let p = piece.trim();
        let Some((name, ty)) = p.split_once(':') else {
            continue;
        };
        names.push(name.trim().to_string());
        types.push(ty.trim().to_string());
    }
    (names, types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_counterexample_lines() {
        let text = "Counterexample\n  x1: 9\n  x0: 7\n";
        let params = vec![RustMetaParam { bits: 32 }, RustMetaParam { bits: 8 }];
        let parsed = parse_counterexample_bindings(text, &params);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].index, 0);
        assert_eq!(parsed[0].value, 7);
        assert_eq!(parsed[1].bits, 8);
    }

    #[test]
    fn parses_rust_fn_signature() {
        let src = "pub fn add_one(x: u32, y: bool) -> u32 { x + (y as u32) }";
        let (names, types) = parse_rust_signature(src, "add_one");
        assert_eq!(names, vec!["x", "y"]);
        assert_eq!(types, vec!["u32", "bool"]);
    }

    #[test]
    fn capture_allow_failure_keeps_output() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg("echo Counterexample && exit 2");
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg("echo Counterexample; exit 2");
            c
        };
        let out = run_capture_allow_failure(&mut cmd).expect("capture should succeed");
        assert!(out.contains("Counterexample"));
    }
}
