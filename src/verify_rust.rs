use crate::result_json::{write_verify_result, ResultCounterexample, VerifyResultJson};
use crate::tool_discovery::discover_tools;
use crate::verify_rust_helpers::{
    eval_cryptol_at_cex, parse_counterexample_bindings, parse_rust_signature, run_and_print,
    run_capture, run_rust_harness_at_cex, RustMetaParam,
};
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
    params: Vec<RustMetaParam>,
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
