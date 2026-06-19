use crate::result_json::{write_verify_result, ResultCounterexample, VerifyResultJson};
use crate::tool_discovery::discover_tools;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct VerifyRustArgs {
    pub rust_file: PathBuf,
    pub cryptol_spec: PathBuf,
    pub cryptol_fn: String,
    pub function: String,
    pub output_dir: Option<PathBuf>,
    pub spec_only_on_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyRustOutcome {
    Verified,
    NotAttempted,
    Disproved,
    Unknown,
}

impl VerifyRustOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            VerifyRustOutcome::Verified => 0,
            VerifyRustOutcome::NotAttempted => 0,
            VerifyRustOutcome::Disproved => 1,
            VerifyRustOutcome::Unknown => 2,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RustMeta {
    mangled_name: String,
    params: Vec<RustMetaParam>,
}

#[derive(Debug, Deserialize)]
struct RustMetaParam {
    index: usize,
    name: String,
    bits: u32,
    llvm_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CexBinding {
    index: usize,
    name: String,
    value: u64,
    bits: u32,
}

pub fn run(args: VerifyRustArgs) -> Result<VerifyRustOutcome> {
    let rust_file = fs::canonicalize(&args.rust_file)
        .with_context(|| format!("failed to resolve {}", args.rust_file.display()))?;
    let cryptol_spec = fs::canonicalize(&args.cryptol_spec)
        .with_context(|| format!("failed to resolve {}", args.cryptol_spec.display()))?;
    let base_name = rust_file
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("rust file has no base name"))?
        .to_string();

    let default_out = rust_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("out_rust_{base_name}"));
    let output_dir = args.output_dir.clone().unwrap_or(default_out);
    if output_dir.exists() {
        fs::remove_dir_all(&output_dir)
            .with_context(|| format!("failed to remove {}", output_dir.display()))?;
    }
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let output_dir = fs::canonicalize(output_dir)?;

    let tools = discover_tools()?;
    tools.require_verify_rust()?;
    tools.add_solver_and_llvm_to_path();

    let rustc = tools.rustc.as_ref().expect("required above");
    let llvm_dis = tools.llvm_dis.as_ref().expect("required above");
    let saw = tools.saw.as_ref().expect("required above");

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Step 1: rustc {base_name}.rs → bitcode");
    println!("═══════════════════════════════════════════════════════");

    let bc_file = output_dir.join(format!("{base_name}.bc"));
    let rust_out = output_dir.join(format!("{base_name}.out"));
    let emit_arg = format!("llvm-bc={}", bc_file.display());
    run_and_print(
        Command::new(rustc)
            .arg("--emit")
            .arg(emit_arg)
            .arg("--crate-type=lib")
            .arg("--edition=2021")
            .arg("--target")
            .arg(&tools.llvm_target)
            .arg("-C")
            .arg("opt-level=0")
            .arg("-C")
            .arg("link-dead-code=yes")
            .arg("-C")
            .arg("symbol-mangling-version=v0")
            .arg("-C")
            .arg("overflow-checks=off")
            .arg("-C")
            .arg("debug-assertions=off")
            .arg("-C")
            .arg("panic=unwind")
            .arg("-C")
            .arg("codegen-units=1")
            .arg("-C")
            .arg("debuginfo=0")
            .arg("-C")
            .arg("lto=off")
            .arg("-C")
            .arg("embed-bitcode=no")
            .arg("-o")
            .arg(rust_out)
            .arg(&rust_file),
    )?;
    if !bc_file.exists() {
        bail!("rustc did not produce {}", bc_file.display());
    }
    println!(
        "  → {} ({} bytes)",
        bc_file.display(),
        fs::metadata(&bc_file)?.len()
    );

    maybe_lower_exceptions(&tools, &bc_file, &output_dir, &base_name)?;

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Step 2: llvm-dis → {base_name}.ll");
    println!("═══════════════════════════════════════════════════════");
    let ll_file = output_dir.join(format!("{base_name}.ll"));
    run_and_print(Command::new(llvm_dis).arg(&bc_file).arg("-o").arg(&ll_file))?;
    if !ll_file.exists() {
        bail!("llvm-dis failed to produce {}", ll_file.display());
    }
    println!("  → {}", ll_file.display());

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Step 3: saw-spec-gen gen-verify-rust");
    println!("═══════════════════════════════════════════════════════");
    let current_exe =
        std::env::current_exe().context("failed to locate current saw-spec-gen exe")?;
    let mut gen_cmd = Command::new(&current_exe);
    gen_cmd
        .arg("gen-verify-rust")
        .arg("--llvm-ir")
        .arg(&ll_file)
        .arg("--bitcode")
        .arg(&bc_file)
        .arg("--cryptol-spec")
        .arg(&cryptol_spec)
        .arg("--cryptol-fn")
        .arg(&args.cryptol_fn)
        .arg("--function")
        .arg(&args.function)
        .arg("--output")
        .arg(&output_dir);
    if args.spec_only_on_missing {
        gen_cmd.arg("--spec-only-on-missing");
    }
    run_and_print(&mut gen_cmd)?;

    let saw_script = output_dir.join("verify_rust.saw");
    let meta_path = output_dir.join("verify_rust.meta.json");
    if !saw_script.exists() || !meta_path.exists() {
        if args.spec_only_on_missing && output_dir.join("result.json").exists() {
            println!("RESULT: NOT_ATTEMPTED");
            return Ok(VerifyRustOutcome::NotAttempted);
        }
        if !saw_script.exists() {
            bail!("missing {}", saw_script.display());
        }
        if !meta_path.exists() {
            bail!("missing {}", meta_path.display());
        }
    }
    let meta: RustMeta = serde_json::from_str(
        &fs::read_to_string(&meta_path)
            .with_context(|| format!("failed to read {}", meta_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", meta_path.display()))?;
    let _mangled_name = meta.mangled_name.clone();
    let _meta_debug: Vec<(usize, String, String)> = meta
        .params
        .iter()
        .map(|p| (p.index, p.name.clone(), p.llvm_type.clone()))
        .collect();

    let rust_src = fs::read_to_string(&rust_file)
        .with_context(|| format!("failed to read {}", rust_file.display()))?;
    let (rust_param_names, rust_param_types) = parse_rust_signature(&rust_src, &args.function);

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Step 4: SAW verification");
    println!("═══════════════════════════════════════════════════════");
    let saw_out = run_capture(
        Command::new(saw)
            .arg("verify_rust.saw")
            .current_dir(&output_dir),
    )?;
    print!("{saw_out}");

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Result");
    println!("═══════════════════════════════════════════════════════");

    if saw_out.contains("Counterexample") {
        let mut cex_pairs = parse_counterexample_bindings(&saw_out, &meta.params);
        for p in &mut cex_pairs {
            if p.index < rust_param_names.len() {
                p.name = rust_param_names[p.index].clone();
            }
        }
        let expected = eval_cryptol_at_cex(
            saw,
            &output_dir,
            &cryptol_spec,
            &args.cryptol_fn,
            &cex_pairs,
        )?;
        let actual = run_rust_harness_at_cex(
            rustc,
            &tools.llvm_target,
            tools.exe_ext,
            &output_dir,
            &rust_file,
            &args.function,
            &rust_param_types,
            &cex_pairs,
        )?;

        println!();
        println!("  RESULT: DISPROVED");
        println!("    Rust {}  ≢  {}", args.function, args.cryptol_fn);
        if !cex_pairs.is_empty() {
            println!();
            println!("  Counterexample input:");
            for p in &cex_pairs {
                println!("    {:<8} = {}", p.name, p.value);
            }
        }
        if expected.is_some() || actual.is_some() {
            let display_args = cex_pairs
                .iter()
                .map(|p| p.value.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!();
            println!("  At this input:");
            if let Some(exp) = &expected {
                println!(
                    "    Cryptol  {}({}) = {}",
                    args.cryptol_fn, display_args, exp
                );
            }
            if let Some(act) = &actual {
                let marker = if expected.as_ref() == Some(act) {
                    "✓"
                } else {
                    "✗"
                };
                println!(
                    "    Rust     {}({}) = {}  {}",
                    args.function, display_args, act, marker
                );
            }
        }
        println!();
        println!("  Output dir: {}", output_dir.display());
        println!();
        println!("RESULT: DISPROVED");

        write_result(
            &output_dir,
            &args,
            "DISPROVED",
            cex_pairs
                .iter()
                .map(|p| ResultCounterexample {
                    name: p.name.clone(),
                    value: Some(p.value.to_string()),
                    bits: Some(p.bits),
                })
                .collect(),
            expected,
            actual,
            &rust_file,
        )?;
        update_inventory(&current_exe, &output_dir)?;
        return Ok(VerifyRustOutcome::Disproved);
    }

    if saw_out.contains("VERIFIED") {
        println!("  RESULT: VERIFIED");
        println!(
            "    Rust {}  ≡  {} (for all u32 inputs)",
            args.function, args.cryptol_fn
        );
        println!("  Output dir: {}", output_dir.display());
        println!();
        println!("RESULT: VERIFIED");
        write_result(
            &output_dir,
            &args,
            "VERIFIED",
            vec![],
            None,
            None,
            &rust_file,
        )?;
        update_inventory(&current_exe, &output_dir)?;
        return Ok(VerifyRustOutcome::Verified);
    }

    println!("  RESULT: UNKNOWN — could not classify SAW output");
    println!(
        "  Inspect {} for the .saw script + raw SAW output.",
        output_dir.display()
    );
    println!();
    println!("RESULT: UNKNOWN");
    write_result(
        &output_dir,
        &args,
        "UNKNOWN",
        vec![],
        None,
        None,
        &rust_file,
    )?;
    update_inventory(&current_exe, &output_dir)?;
    Ok(VerifyRustOutcome::Unknown)
}

fn maybe_lower_exceptions(
    tools: &crate::tool_discovery::DiscoveredTools,
    bc_file: &Path,
    output_dir: &Path,
    base_name: &str,
) -> Result<()> {
    let is_msvc = tools.llvm_target.contains("windows-msvc");
    let Some(exception_lower) = tools.exception_lower.as_ref() else {
        if is_msvc {
            println!(
                "  note: exception-lower binary not available; EH / cleanup funclets will not be lowered."
            );
        }
        return Ok(());
    };

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" Step 1.25: Lower exception handling");
    println!("═══════════════════════════════════════════════════════");
    let lowered_bc = output_dir.join(format!("{base_name}_lowered.bc"));
    let out = run_capture(
        Command::new(exception_lower)
            .arg(bc_file)
            .arg("-o")
            .arg(&lowered_bc),
    );
    match out {
        Ok(text) => {
            print!("{text}");
            if lowered_bc.exists() {
                fs::copy(&lowered_bc, bc_file)?;
                println!("  → {} (lowered)", bc_file.display());
            } else {
                println!("  warning: exception-lower failed; continuing with unlowered IR");
            }
        }
        Err(err) => {
            println!("  warning: exception-lower failed; continuing with unlowered IR");
            println!("  detail: {err}");
        }
    }
    Ok(())
}

fn run_and_print(cmd: &mut Command) -> Result<()> {
    let text = run_capture(cmd)?;
    print!("{text}");
    Ok(())
}

fn run_capture(cmd: &mut Command) -> Result<String> {
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

fn write_result(
    output_dir: &Path,
    args: &VerifyRustArgs,
    verdict: &str,
    counterexample: Vec<ResultCounterexample>,
    expected: Option<String>,
    actual: Option<String>,
    rust_file: &Path,
) -> Result<()> {
    let mut payload = VerifyResultJson::new("rust", &args.function, &args.cryptol_fn, verdict);
    payload.counterexample = counterexample;
    payload.expected = expected;
    payload.actual = actual;
    payload.solver = Some("z3".to_string());
    payload.impl_file = rust_file
        .file_name()
        .and_then(|s| s.to_str())
        .map(ToOwned::to_owned);
    write_verify_result(output_dir, &payload)
}

fn update_inventory(current_exe: &Path, output_dir: &Path) -> Result<()> {
    let Some(root) = output_dir.parent() else {
        return Ok(());
    };
    let inventory_out = root.join("implementation_inventory.json");
    let out = run_capture(
        Command::new(current_exe)
            .arg("aggregate-inventory")
            .arg(root)
            .arg("--output")
            .arg(&inventory_out),
    );
    match out {
        Ok(text) => {
            print!("{text}");
            if inventory_out.exists() {
                println!("  → {}", inventory_out.display());
            }
        }
        Err(err) => {
            println!(
                "warning: failed to aggregate implementation inventory at {} ({err})",
                inventory_out.display()
            );
        }
    }
    Ok(())
}

fn parse_counterexample_bindings(saw_out: &str, params: &[RustMetaParam]) -> Vec<CexBinding> {
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

fn eval_cryptol_at_cex(
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
fn run_rust_harness_at_cex(
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

fn parse_rust_signature(src: &str, function: &str) -> (Vec<String>, Vec<String>) {
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
        let params = vec![
            RustMetaParam {
                index: 0,
                name: "x0".to_string(),
                bits: 32,
                llvm_type: "i32".to_string(),
            },
            RustMetaParam {
                index: 1,
                name: "x1".to_string(),
                bits: 8,
                llvm_type: "i8".to_string(),
            },
        ];
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
}
