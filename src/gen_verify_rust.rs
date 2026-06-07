//! Implements the `gen-verify-rust` subcommand.
//!
//! Produces a SAW verification script + meta sidecar that lets
//! `verify-rust.ps1` prove a Rust function matches a hand-written
//! Cryptol spec. Supports sret, aggregate returns, range preconditions,
//! buffer overrides, `--spec-only-on-missing`, and `--async` (coroutine
//! resume verification via `mir_verify`).

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::buffer_overrides::BufferOverrides;
use crate::constraints::TypeInfo;
use crate::gen_verify_helpers::emit_spec_only_result;
use crate::gen_verify_rust_async;
use crate::gen_verify_rust_emit::{self, RustReturnKind, RustVerifyArtifacts, RustVerifyParam};
use crate::parsers::llvm_ir::extract_functions;

/// Run the `gen-verify-rust` subcommand. Writes `verify_rust.saw` and
/// `verify_rust.meta.json` into `output`, and copies the Cryptol spec.
///
/// When `is_async` is `true` the command targets the coroutine resume
/// (`_RNC…`) and emits `mir_verify`; `bitcode` should then be the
/// `.linked-mir.json` from `cargo-saw-build` / `mir-json --link-mir`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    llvm_ir: &Path,
    bitcode: &Path,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
    output: &Path,
    spec_only_on_missing: bool,
    overrides: &BufferOverrides,
    variant_map: &gen_verify_rust_emit::VariantMap,
    is_async: bool,
) -> Result<()> {
    if overrides.has_auto_out_buffers() {
        bail!(
            "--out-buffer-param NAME=auto is currently supported only on the C++ gen-verify path"
        );
    }
    fs::create_dir_all(output)
        .with_context(|| format!("creating output dir {}", output.display()))?;

    let ir =
        fs::read_to_string(llvm_ir).with_context(|| format!("reading {}", llvm_ir.display()))?;

    let cry_name = cryptol_spec
        .file_name()
        .ok_or_else(|| anyhow!("--cryptol-spec has no file name"))?
        .to_string_lossy()
        .into_owned();
    let cry_dest = output.join(&cry_name);
    fs::copy(cryptol_spec, &cry_dest).with_context(|| {
        format!(
            "copying {} → {}",
            cryptol_spec.display(),
            cry_dest.display()
        )
    })?;

    let bc_name = bitcode
        .file_name()
        .ok_or_else(|| anyhow!("--bitcode has no file name"))?
        .to_string_lossy()
        .into_owned();

    if is_async {
        return run_async(
            &ir,
            function,
            cryptol_fn,
            &bc_name,
            &cry_name,
            output,
            spec_only_on_missing,
        );
    }

    let arts = match resolve_target(&ir, function) {
        Ok(a) => a,
        Err(e) if spec_only_on_missing => {
            return emit_spec_only_result(output, cryptol_fn, function, &format!("{e:#}"));
        }
        Err(e) => return Err(e),
    };

    let saw_path = output.join("verify_rust.saw");
    let saw_text = gen_verify_rust_emit::emit_saw_script(
        function,
        cryptol_fn,
        &bc_name,
        &cry_name,
        &arts,
        overrides,
        variant_map,
    );
    fs::write(&saw_path, saw_text).with_context(|| format!("writing {}", saw_path.display()))?;

    let meta_path = output.join("verify_rust.meta.json");
    let meta = json!({
        "schema_version": 1,
        "function": function,
        "cryptol_fn": cryptol_fn,
        "mangled_name": arts.mangled_name,
        "params": arts.params.iter().map(|p| json!({
            "index": p.index,
            "name": p.name,
            "bits": p.bits,
            "llvm_type": p.llvm_type,
        })).collect::<Vec<_>>(),
        "return_bits": arts.return_bits(),
        "return_llvm_type": arts.return_llvm_type,
        "globals": arts.globals,
    });
    let mut meta_text = serde_json::to_string_pretty(&meta)?;
    meta_text.push('\n');
    fs::write(&meta_path, meta_text).with_context(|| format!("writing {}", meta_path.display()))?;

    println!("  → mangled symbol: {}", arts.mangled_name);
    let sig = arts
        .params
        .iter()
        .map(|p| p.llvm_type.clone())
        .collect::<Vec<_>>()
        .join(", ");
    println!("  → LLVM signature: ({sig})");
    println!("  → LLVM return type: {}", arts.return_llvm_type);
    if !arts.globals.is_empty() {
        println!("  → allocating {} mutable global(s):", arts.globals.len());
        for g in &arts.globals {
            println!("      {g}");
        }
    }
    println!("  → wrote {}", saw_path.display());
    println!("  → wrote {}", meta_path.display());
    Ok(())
}

/// `--async` handler: resolves coroutine resume, emits `mir_verify` script.
fn run_async(
    ir: &str,
    function: &str,
    cryptol_fn: &str,
    mir_module: &str,
    cry_name: &str,
    output: &Path,
    spec_only_on_missing: bool,
) -> Result<()> {
    let constructor = match resolve_target(ir, function) {
        Ok(a) => a,
        Err(e) if spec_only_on_missing => {
            return emit_spec_only_result(output, cryptol_fn, function, &format!("{e:#}"));
        }
        Err(e) => return Err(e),
    };

    let return_bits = constructor.return_bits();
    let resume_info = match gen_verify_rust_async::resolve_resume_symbol(
        ir,
        function,
        constructor.params,
        return_bits,
    ) {
        Ok(i) => i,
        Err(e) if spec_only_on_missing => {
            return emit_spec_only_result(output, cryptol_fn, function, &format!("{e:#}"));
        }
        Err(e) => return Err(e),
    };

    let saw_path = output.join("verify_rust.saw");
    let saw_text = gen_verify_rust_async::emit_async_mir_saw_script(
        function,
        cryptol_fn,
        mir_module,
        cry_name,
        &resume_info,
    );
    fs::write(&saw_path, &saw_text).with_context(|| format!("writing {}", saw_path.display()))?;

    let meta_path = output.join("verify_rust.meta.json");
    let meta = json!({
        "schema_version": 1,
        "function": function,
        "cryptol_fn": cryptol_fn,
        "async": true,
        "resume_symbol": resume_info.resume_symbol,
        "params": resume_info.original_params.iter().map(|p| json!({
            "index": p.index,
            "name": p.name,
            "bits": p.bits,
            "llvm_type": p.llvm_type,
        })).collect::<Vec<_>>(),
        "return_bits": resume_info.return_bits,
        "sub_await_stubs": resume_info.sub_await_stubs,
    });
    let mut meta_text = serde_json::to_string_pretty(&meta)?;
    meta_text.push('\n');
    fs::write(&meta_path, &meta_text)
        .with_context(|| format!("writing {}", meta_path.display()))?;

    println!("  → async mode: targeting coroutine resume");
    println!("  → resume symbol: {}", resume_info.resume_symbol);
    if !resume_info.sub_await_stubs.is_empty() {
        println!(
            "  → {} .await sub-poll stub(s) detected",
            resume_info.sub_await_stubs.len()
        );
    }
    println!("  → wrote {}", saw_path.display());
    println!("  → wrote {}", meta_path.display());
    Ok(())
}

/// A candidate target function with its resolved type information.
struct ResolvedCandidate {
    name: String,
    params: Vec<RustVerifyParam>,
    return_kind: RustReturnKind,
    return_llvm_type: String,
}

/// Walk the IR text and resolve the mangled symbol for `function`.
/// Accepts integer-param functions with scalar/sret/aggregate return;
/// rejects ptr-only shims. Picks shortest match; bails on a tie.
fn resolve_target(ir: &str, function: &str) -> Result<RustVerifyArtifacts> {
    let funcs = extract_functions(ir, None).context("parsing LLVM IR functions")?;
    let suffix = format!("{}{}", function.len(), function);

    let name_matches: Vec<_> = funcs
        .iter()
        .filter(|f| f.has_body && f.name.ends_with(&suffix))
        .collect();
    if name_matches.is_empty() {
        bail!(
            "Could not find a defined function whose mangled name ends with '{}'.\n\
             Make sure --llvm-ir points at the disassembled bitcode for the same\n\
             crate that defines `{}`.",
            suffix,
            function
        );
    }

    let range_map = parse_range_attrs(ir, &suffix);

    let mut candidates: Vec<ResolvedCandidate> = Vec::new();
    for f in &name_matches {
        if let Some(c) = try_resolve_candidate(f, &range_map) {
            candidates.push(c);
        }
    }
    if candidates.is_empty() {
        let names = name_matches
            .iter()
            .map(|f| format!("  {}", f.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "Found symbol(s) ending in '{}' but none have a compatible \
             signature for this verifier.\nCandidates were:\n{}",
            suffix,
            names
        );
    }
    candidates.sort_by_key(|c| c.name.len());
    if candidates.len() >= 2 && candidates[0].name.len() == candidates[1].name.len() {
        let shortest = candidates[0].name.len();
        let tied = candidates
            .iter()
            .take_while(|c| c.name.len() == shortest)
            .map(|c| format!("  {}", c.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "Ambiguous '{}': multiple matching symbols of equal length. \
             Qualify the function name (e.g. `mod::{}`) or rename one.\n\
             Tied candidates:\n{}",
            function,
            function,
            tied
        );
    }

    let chosen = candidates.into_iter().next().unwrap();
    let globals = scan_globals(ir);
    Ok(RustVerifyArtifacts {
        mangled_name: chosen.name,
        params: chosen.params,
        return_kind: chosen.return_kind,
        return_llvm_type: chosen.return_llvm_type,
        globals,
    })
}

/// Build a [`ResolvedCandidate`]; returns `None` for incompatible signatures.
fn try_resolve_candidate(
    f: &crate::constraints::FunctionInfo,
    range_map: &std::collections::HashMap<String, Vec<Option<(u64, u64)>>>,
) -> Option<ResolvedCandidate> {
    let ranges = range_map.get(&f.name);
    let mut params = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        let bits = type_int_bits(&p.ty)?;
        let range = ranges.and_then(|r| r.get(i).copied().flatten());
        params.push(RustVerifyParam {
            index: i,
            name: format!("x{i}"),
            bits,
            llvm_type: format!("i{bits}"),
            range,
        });
    }
    let return_kind = classify_return(&f.return_type)?;
    let return_llvm_type = format_return_llvm_type(&return_kind);
    Some(ResolvedCandidate {
        name: f.name.clone(),
        params,
        return_kind,
        return_llvm_type,
    })
}

fn classify_return(ty: &TypeInfo) -> Option<RustReturnKind> {
    match ty {
        TypeInfo::Bool => Some(RustReturnKind::Scalar { bits: 1 }),
        TypeInfo::SignedInt(n) | TypeInfo::UnsignedInt(n) => {
            Some(RustReturnKind::Scalar { bits: *n })
        }
        TypeInfo::Struct {
            name,
            size_bytes,
            fields,
        } => {
            let field_bits: Option<Vec<u32>> =
                fields.iter().map(|(_, ft)| type_int_bits(ft)).collect();
            if let Some(fb) = field_bits {
                return Some(RustReturnKind::Aggregate { field_bits: fb });
            }
            if let Some(sz) = size_bytes {
                if *sz > 0 {
                    return Some(RustReturnKind::Sret {
                        llvm_type: format!("%{name}"),
                        size_bytes: *sz,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

fn format_return_llvm_type(kind: &RustReturnKind) -> String {
    match kind {
        RustReturnKind::Scalar { bits } => format!("i{bits}"),
        RustReturnKind::Sret { llvm_type, .. } => llvm_type.clone(),
        RustReturnKind::Aggregate { field_bits } => {
            let fields = field_bits
                .iter()
                .map(|b| format!("i{b}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
    }
}

/// Width in bits of an `iN`-shaped [`TypeInfo`], or `None`.
fn type_int_bits(t: &TypeInfo) -> Option<u32> {
    match t {
        TypeInfo::Bool => Some(1),
        TypeInfo::SignedInt(n) | TypeInfo::UnsignedInt(n) => Some(*n),
        _ => None,
    }
}

/// Parse `range(lo, hi)` attributes from IR define lines matching `suffix`.
fn parse_range_attrs(
    ir: &str,
    suffix: &str,
) -> std::collections::HashMap<String, Vec<Option<(u64, u64)>>> {
    let mut map = std::collections::HashMap::new();
    for line in ir.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("define ") {
            continue;
        }
        let Some(at_pos) = trimmed.find('@') else {
            continue;
        };
        let Some(paren_pos) = trimmed[at_pos..].find('(') else {
            continue;
        };
        let name = &trimmed[at_pos + 1..at_pos + paren_pos];
        if !name.ends_with(suffix) {
            continue;
        };
        let open = at_pos + paren_pos;
        let Some(close) = find_matching_close(trimmed, open) else {
            continue;
        };
        let params_str = &trimmed[open + 1..close];
        let ranges = extract_param_ranges(params_str);
        map.insert(name.to_string(), ranges);
    }
    map
}

/// Find closing `)` that matches the opening `(` at `open_pos`.
fn find_matching_close(s: &str, open_pos: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, ch) in s[open_pos..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_pos + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract `range(lo, hi)` from a comma-separated parameter list.
fn extract_param_ranges(params_str: &str) -> Vec<Option<(u64, u64)>> {
    if params_str.trim().is_empty() {
        return vec![];
    }
    split_ir_params(params_str)
        .iter()
        .map(|p| extract_single_range(p))
        .collect()
}

fn extract_single_range(param: &str) -> Option<(u64, u64)> {
    let idx = param.find("range(")?;
    let rest = &param[idx + 6..];
    let close = rest.find(')')?;
    let inner = &rest[..close];
    let mut parts = inner.split(',');
    let lo: u64 = parts.next()?.trim().parse().ok()?;
    let hi: u64 = parts.next()?.trim().parse().ok()?;
    Some((lo, hi))
}

/// Split IR parameter list on commas, respecting parenthesized groups.
fn split_ir_params(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

/// Scan the IR for mutable `global` definitions.
fn scan_globals(ir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for line in ir.lines() {
        let line = line.trim_start();
        if !line.starts_with('@') {
            continue;
        }
        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        let head = line[1..eq_pos].trim();
        if head.is_empty() {
            continue;
        }
        let mut name = head.to_string();
        if name.starts_with('"') && name.ends_with('"') && name.len() >= 2 {
            name = name[1..name.len() - 1].to_string();
        }
        if name.contains("__imp_") {
            continue;
        }
        let tail = line[eq_pos + 1..].trim_start();
        if !contains_global_keyword(tail) {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

fn contains_global_keyword(tail: &str) -> bool {
    for tok in tail.split_whitespace() {
        let is_word = tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_word {
            return false;
        }
        match tok {
            "global" => return true,
            "constant" | "alias" | "ifunc" => return false,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
#[path = "gen_verify_rust_tests.rs"]
mod tests;
