//! Native `saw-spec-gen verify-cpp` implementation for C++ targets.

mod cache_fingerprint;
mod compile;
mod counterexample;

use crate::inventory;
use crate::project_config::MergedConfig;
use crate::verify_cache::AstCacheContext;
use crate::verify_result::{write_verify_result, ContractClause, FunctionContract};
use crate::verify_tools::ToolPaths;
use anyhow::{bail, Context, Result};
use compile::{
    build_clang_flags, dump_ast, emit_bitcode, emit_llvm_ir, is_spec_only_result, maybe_filter_ast,
    maybe_lower_exceptions, patch_ir_and_reassemble, recompile_at_o1, recompile_promoted,
    run_gen_verify, O1Recompile, PromoteRecompile,
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
    pub config: Option<PathBuf>,
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
    let config = req
        .config
        .as_ref()
        .map(|p| {
            p.canonicalize()
                .with_context(|| format!("failed to resolve config {}", p.display()))
        })
        .transpose()?;
    // Resolve `spec_only_on_missing` from config (explicit path or
    // spec-relative auto-discovery). The forked `gen-verify` subprocess
    // does its own config discovery for shaping; this local copy only
    // drives verify-cpp's own soft-exit decision below.
    let merged_config = {
        let cwd = std::env::current_dir()?;
        match config.as_deref() {
            Some(p) => crate::project_config::ProjectConfig::load(p)?,
            None => crate::project_config::ProjectConfig::discover_for_spec(&cryptol_spec, &cwd)?,
        }
        .apply(&req.cryptol_fn)
    };
    let spec_only_on_missing = merged_config.spec_only_on_missing;
    let contract = function_contract(&req.cryptol_fn, &merged_config)?;
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

    let mut cache_flags = user_clang_flags.clone();
    cache_flags.push(cache_fingerprint::fingerprint(
        clang,
        tools.llvm_target,
        &user_clang_flags,
        &cpp_file,
    )?);

    let cache = AstCacheContext::load(
        &cpp_file,
        &include_dirs,
        &cache_flags,
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
        &cryptol_spec,
        &req.cryptol_fn,
        &req.function,
        config.as_deref(),
    )?;
    if spec_only_on_missing && is_spec_only_result(&output_dir)? {
        eprintln!(
            "spec-only: no C++ implementation for '{}' — skipping SAW.",
            req.function
        );
        return Ok(VerifyOutcome { exit_code: 0 });
    }

    let saw_started = Instant::now();
    let mut saw_output = run_saw(saw, &output_dir, "verify.saw")?;
    // Part 2 (§2): the `-O0` STL build can emit constructs SAW's
    // simulator cannot execute — empty-struct global loads from
    // `std::optional` / `std::nullopt_t` ctors, and uninitialized reads
    // of stateless-functor allocas (`std::equal_to` inside `std::equal`,
    // the optional copy path in `provision`) — which abort with a vacuous
    // `Error during memory load`. Crucible still prints a `<<All settings
    // ...>>` counterexample block for these, so the presence of
    // "Counterexample" is *not* a reliable conclusiveness signal here;
    // gate the recovery on the memory-load signature itself (via
    // [`is_empty_struct_load_failure`]) plus the absence of a real
    // VERIFIED. A genuine z3 DISPROVED never emits that signature, so a
    // real verdict is never overridden.
    //
    // Recovery is two-tier. First try an in-place promotion that strips
    // `optnone` and runs `sroa,mem2reg,instsimplify` (see
    // [`recompile_promoted`]): this rewrites the offending uninitialized
    // reads to `undef` registers *without inlining*, so mutex / memcmp
    // overrides stay intact and the proof reaches its real obligation.
    // Only if promotion is unavailable or leaves the result inconclusive
    // (no verdict AND no counterexample) do we fall back to the heavier
    // `-O1` recompile, which inlines the STL bodies at the cost of also
    // inlining the MSVC mutex ownership check into an `unreachable`.
    if !saw_output.contains("VERIFIED") && is_empty_struct_load_failure(&saw_output) {
        eprintln!(
            "note: SAW could not load the -O0 build (uninitialized empty-struct/tag load); \
             retrying with optnone-strip + opt promotion."
        );
        let promoted = recompile_promoted(&PromoteRecompile {
            tools: &tools,
            llvm_as,
            output_dir: &output_dir,
            base_name: &base_name,
            bc_file: &bc_file,
            ll_file: &ll_file,
            ast_file: &ast_file,
            cryptol_spec: &cryptol_spec,
            cryptol_fn: &req.cryptol_fn,
            function: &req.function,
            config: config.as_deref(),
        })?;
        if promoted {
            saw_output = run_saw(saw, &output_dir, "verify.saw")?;
        }
        // Fall back to -O1 only if promotion was unavailable or left the
        // result without a conclusive verdict. A Counterexample produced
        // on the promoted (un-inlined) build is a trustworthy DISPROVED
        // and must not be overridden by the mutex-breaking -O1 build.
        let inconclusive =
            !saw_output.contains("VERIFIED") && !saw_output.contains("Counterexample");
        if inconclusive {
            eprintln!("note: promoted build still inconclusive; retrying once at -O1.");
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
                cryptol_spec: &cryptol_spec,
                cryptol_fn: &req.cryptol_fn,
                function: &req.function,
                config: config.as_deref(),
            })?;
            saw_output = run_saw(saw, &output_dir, "verify.saw")?;
        }
    }
    let time_secs = saw_started.elapsed().as_secs_f64();
    let impl_file = cpp_file.file_name().and_then(OsStr::to_str);
    if saw_output.contains("Counterexample") {
        let counterexample = parse_counterexample(&saw_output);
        let (expected, actual) = if output_dir.join("layout-plan.json").exists() {
            // Scalar replay cannot represent named aggregate inputs or pointer
            // provenance. SAW's field-level counterexample is authoritative.
            (None, None)
        } else {
            evaluate_counterexample(
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
            )?
        };
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
            &contract,
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
            &contract,
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
        &contract,
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

/// Describe every SAW assertion as a clause of one implementation-function
/// contract. Legacy `cryptol_fn_out` bindings remain valid syntax, but their
/// model functions are recorded only as clause provenance under this contract.
fn function_contract(cryptol_fn: &str, config: &MergedConfig) -> Result<FunctionContract> {
    let mut clauses = vec![ContractClause {
        name: "return".to_string(),
        assertion: "llvm_return".to_string(),
        region: None,
        cryptol_fn: cryptol_fn.to_string(),
        projection: config.contract_return.as_deref().map(normalize_projection),
    }];
    for binding in &config.cryptol_fn_out {
        let (region, source) = split_binding(binding, "cryptol_fn_out")?;
        clauses.push(memory_clause(region, source, None));
    }
    for binding in &config.contract_ensures {
        let (region, projection) = split_binding(binding, "contract_ensures")?;
        clauses.push(memory_clause(
            region,
            cryptol_fn,
            Some(normalize_projection(projection)),
        ));
    }
    Ok(FunctionContract { clauses })
}

fn split_binding<'a>(binding: &'a str, key: &str) -> Result<(&'a str, &'a str)> {
    let (left, right) = binding
        .split_once('=')
        .with_context(|| format!("{key} entry must be REGION=VALUE, got {binding:?}"))?;
    let (left, right) = (left.trim(), right.trim());
    if left.is_empty() || right.is_empty() {
        bail!("{key} entry must be REGION=VALUE, got {binding:?}");
    }
    Ok((left, right))
}

fn normalize_projection(field: &str) -> String {
    field.trim().trim_start_matches('.').to_string()
}

fn memory_clause(region: &str, source: &str, projection: Option<String>) -> ContractClause {
    ContractClause {
        name: region.to_string(),
        assertion: "llvm_points_to".to_string(),
        region: Some(region.to_string()),
        cryptol_fn: source.to_string(),
        projection,
    }
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
    for line in String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|line| line.starts_with("PROOF SCOPE WARNING:"))
    {
        eprintln!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{function_contract, is_empty_struct_load_failure};
    use crate::project_config::ProjectConfig;

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

    #[test]
    fn records_single_model_contract_clause_provenance() {
        let cfg: ProjectConfig = toml::from_str(
            r#"[functions.bump]
contract_return = "ret"
contract_ensures = ["out=outPost"]"#,
        )
        .unwrap();
        let contract = function_contract("bump", &cfg.apply("bump")).unwrap();
        assert_eq!(contract.clauses.len(), 2);
        assert_eq!(contract.clauses[0].cryptol_fn, "bump");
        assert_eq!(contract.clauses[0].projection.as_deref(), Some("ret"));
        assert_eq!(contract.clauses[1].name, "out");
        assert_eq!(contract.clauses[1].cryptol_fn, "bump");
        assert_eq!(contract.clauses[1].projection.as_deref(), Some("outPost"));
    }

    #[test]
    fn legacy_split_model_is_one_contract_with_clause_sources() {
        let cfg: ProjectConfig = toml::from_str(
            r#"[functions.activateRet]
cryptol_fn_out = ["this=activatePost"]"#,
        )
        .unwrap();
        let contract = function_contract("activateRet", &cfg.apply("activateRet")).unwrap();
        assert_eq!(contract.clauses.len(), 2);
        assert_eq!(contract.clauses[0].cryptol_fn, "activateRet");
        assert_eq!(contract.clauses[1].cryptol_fn, "activatePost");
        assert_eq!(contract.clauses[1].region.as_deref(), Some("this"));
    }
}
