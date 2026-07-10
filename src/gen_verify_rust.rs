//! Implements the `gen-verify-rust` subcommand.
//!
//! Produces a SAW verification script + meta sidecar that lets
//! `verify-rust.ps1` prove a Rust function matches a hand-written
//! Cryptol spec. Now supports sret, aggregate returns, range
//! preconditions, buffer overrides, and `--spec-only-on-missing`.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::buffer_overrides::BufferOverrides;
use crate::gen_verify_helpers::emit_spec_only_result;
use crate::gen_verify_rust_classify::{
    classify_return, classify_sret_return, format_return_llvm_type, type_int_bits,
};
use crate::gen_verify_rust_emit::{self, RustReturnKind, RustVerifyArtifacts, RustVerifyParam};
use crate::inventory;
use crate::parsers::llvm_ir::extract_functions;

/// Run the `gen-verify-rust` subcommand. Writes `verify_rust.saw`
/// and `verify_rust.meta.json` into `output`, and copies the Cryptol
/// spec next to them.
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
    uninterpreted_cfg: &[crate::uninterpreted::UninterpretedEntry],
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

    let arts = match resolve_target(&ir, function) {
        Ok(a) => a,
        Err(e) if spec_only_on_missing => {
            return emit_spec_only_result(output, cryptol_fn, function, &format!("{e:#}"));
        }
        Err(e) => return Err(e),
    };

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

    let saw_path = output.join("verify_rust.saw");
    let uninterp_entries = crate::uninterpreted::gather(cryptol_spec, uninterpreted_cfg);
    let uninterpreted =
        crate::uninterpreted::emit_uninterpreted_block(&uninterp_entries, cryptol_spec);
    let saw_text = gen_verify_rust_emit::emit_saw_script(
        function,
        cryptol_fn,
        &bc_name,
        &cry_name,
        &arts,
        overrides,
        variant_map,
        &uninterpreted,
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

    inventory::emit_fragment(
        output,
        function,
        "rust",
        Some(arts.mangled_name.clone()),
        None,
        cryptol_fn,
        inventory::build_models_note(
            !overrides.max_len_preconds_ordered().is_empty(),
            variant_map.entries.contains_key("return"),
        ),
    )?;

    // Friendly stdout summary mirroring the old PowerShell trace.
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

/// A candidate target function with its resolved type information.
struct ResolvedCandidate {
    name: String,
    params: Vec<RustVerifyParam>,
    return_kind: RustReturnKind,
    return_llvm_type: String,
}

/// Walk the IR text and resolve the mangled symbol whose source name
/// is `function`, plus its full signature + the module's mutable
/// globals.
///
/// 1. Pull every `define …` line via [`extract_functions`].
/// 2. Keep those whose mangled symbol ends with `<len><function>`.
/// 3. Accept functions with integer params and scalar, sret, or
///    aggregate return types. Reject ptr/void-only shims.
/// 4. Pick the shortest remaining symbol; bail on a tie.
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

    // Parse range attributes from raw IR lines for each candidate.
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

/// Try to build a [`ResolvedCandidate`] from a parsed function.
/// Returns `None` if the signature is incompatible (e.g. ptr-only
/// drop glue, void without sret).
fn try_resolve_candidate(
    f: &crate::constraints::FunctionInfo,
    range_map: &std::collections::HashMap<String, Vec<Option<(u64, u64)>>>,
) -> Option<ResolvedCandidate> {
    let ranges = range_map.get(&f.name);

    // Detect a hidden sret output parameter.  After `extract_sret` runs
    // inside the IR parser, the sret param lands at f.params[0] with
    // `WriteOnly` mutability and a non-integer type (ByteArray / Struct /
    // Opaque).  The promoted return type carries the concrete struct type.
    // Note: `extract_sret` strips the `sret` annotation, so we use the
    // WriteOnly + non-integer-type combination as the detection signal.
    let has_sret = f.params.first().is_some_and(|p| {
        // WriteOnly + non-integer type = stripped sret parameter.
        // `extract_sret` removes the Custom("sret") annotation but
        // preserves WriteOnly mutability and the non-integer struct type.
        p.mutability == crate::constraints::Mutability::WriteOnly && type_int_bits(&p.ty).is_none()
    });
    let input_params = if has_sret {
        &f.params[1..]
    } else {
        &f.params[..]
    };
    // IR-level offset so range attributes (indexed over all IR params)
    // are looked up at the right position even after the sret skip.
    let ir_offset = usize::from(has_sret);

    // Collect input params — must all be integer-typed
    let mut params = Vec::new();
    for (local_i, p) in input_params.iter().enumerate() {
        let bits = type_int_bits(&p.ty)?;
        let ir_i = local_i + ir_offset;
        let range = ranges.and_then(|r| r.get(ir_i).copied().flatten());
        params.push(RustVerifyParam {
            index: local_i,
            name: format!("x{local_i}"),
            bits,
            llvm_type: format!("i{bits}"),
            range,
        });
    }
    // Determine return kind
    let return_kind = if has_sret {
        classify_sret_return(&f.return_type)?
    } else {
        classify_return(&f.return_type)?
    };
    let return_llvm_type = format_return_llvm_type(&return_kind);
    Some(ResolvedCandidate {
        name: f.name.clone(),
        params,
        return_kind,
        return_llvm_type,
    })
}

/// Parse `range(lo, hi)` parameter attributes from IR define lines
/// for functions matching `suffix`. Returns a map from mangled name
/// to per-param optional range bounds.
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
        }
        // Find matching close paren (respecting nesting).
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
