//! Uninterpreted-primitive support: emit `llvm_unsafe_assume_spec`
//! contracts for opaque callees (SHA-256, HMAC, AEAD, …) that are
//! infeasible to symbolically execute.
//!
//! saw-spec-gen's default spec is *pure functional equivalence* — it
//! unfolds the whole callee and asserts `f(x) == model(x)`. Some
//! primitives (cryptographic hashes, MACs) cannot be closed in finite
//! time, so instead of unfolding them we bind the symbol to its Cryptol
//! model via an **assumed spec**: the prover reasons about the *caller*
//! compositionally and trusts the primitive's contract.
//!
//! SAW already supports this through `llvm_unsafe_assume_spec`, which
//! applies globally (it does not even need to be in the override list,
//! though we add it for clarity). The only missing piece — supplied
//! here — is *emitting* one from a declaration.
//!
//! ## Declaration surfaces (no CLI flag)
//!
//! **(a) Cryptol-spec annotation** (primary — lives next to the model):
//!
//! ```cryptol
//! /** @uninterpreted */
//! hmacSha256 : [32][8] -> [n][8] -> [32][8]
//!
//! /** @uninterpreted symbol="?HmacSha256@@..." */
//! hmacSha256 : [32][8] -> [n][8] -> [32][8]
//! ```
//!
//! **(b) `saw-spec-gen.toml`** (`[[uninterpreted]]` section, see
//! [`crate::project_config`]):
//!
//! ```toml
//! [[uninterpreted]]
//! cryptol_fn = "hmacSha256"
//! symbol     = "..."          # optional; omit to resolve by name
//! ```
//!
//! Either way the declaration is in git, reviewable in a diff, and
//! self-documenting.

use crate::parsers::cryptol_sig::{self, CrySig, CryType};
use serde::Deserialize;
use std::path::Path;

/// One uninterpreted-primitive declaration. Shared by the TOML config
/// (`[[uninterpreted]]`) and the Cryptol-annotation parser.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UninterpretedEntry {
    /// Cryptol model used as the assumed contract.
    pub cryptol_fn: String,
    /// Implementation symbol to bind. When omitted, the emitter falls
    /// back to `cryptol_fn` (resolve by name / mangling).
    #[serde(default)]
    pub symbol: Option<String>,
}

impl UninterpretedEntry {
    /// The implementation symbol to bind, defaulting to the Cryptol
    /// function name when no explicit override was given.
    pub fn resolved_symbol(&self) -> &str {
        self.symbol.as_deref().unwrap_or(&self.cryptol_fn)
    }
}

/// A SAWScript snippet plus the override handles it defines, ready to be
/// spliced into a verify script and appended to the `llvm_verify`
/// override list.
#[derive(Debug, Default)]
pub struct UninterpretedBlock {
    pub snippet: String,
    pub override_names: Vec<String>,
}

impl UninterpretedBlock {
    pub fn is_empty(&self) -> bool {
        self.override_names.is_empty()
    }
}

/// Constant-time comparison symbols recognized as plain equality.
///
/// `subtle::ct_eq` (Rust) and `CRYPTO_memcmp` / `ConstantTimeEq` (C/C++)
/// implement a branch-free byte compare. They are infeasible to unfold
/// (loop over secret-length data) but their *contract* is just `a == b`,
/// so they are natural candidates for an `@uninterpreted` declaration
/// whose Cryptol model is equality.
pub const CT_COMPARE_SYMBOLS: &[&str] = &["ct_eq", "CRYPTO_memcmp", "ConstantTimeEq"];

/// Whether `symbol` looks like a constant-time comparison primitive.
pub fn is_ct_compare_symbol(symbol: &str) -> bool {
    CT_COMPARE_SYMBOLS
        .iter()
        .any(|needle| symbol.contains(needle))
}

/// Merge the `@uninterpreted` annotations parsed from `cryptol_spec`
/// with the `[[uninterpreted]]` entries from project config.
///
/// Config entries win when both declare the same `cryptol_fn`: an
/// explicit `symbol =` in the TOML overrides the annotation's symbol.
pub fn gather(
    cryptol_spec: &Path,
    config_entries: &[UninterpretedEntry],
) -> Vec<UninterpretedEntry> {
    let text = std::fs::read_to_string(cryptol_spec).unwrap_or_default();
    let mut merged = parse_annotations(&text);
    for cfg in config_entries {
        if let Some(existing) = merged.iter_mut().find(|e| e.cryptol_fn == cfg.cryptol_fn) {
            if cfg.symbol.is_some() {
                existing.symbol = cfg.symbol.clone();
            }
        } else {
            merged.push(cfg.clone());
        }
    }
    merged
}

/// Parse `@uninterpreted` doc-comment markers from Cryptol source text.
///
/// A marker (`@uninterpreted`, optionally followed by `symbol="..."`) in
/// a comment immediately preceding a `name : ...` signature line tags
/// `name` as uninterpreted. Both block (`/** ... */`) and line (`//` /
/// `///`) comment forms are accepted.
pub fn parse_annotations(text: &str) -> Vec<UninterpretedEntry> {
    let mut out = Vec::new();
    let mut pending = false;
    let mut pending_symbol: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.contains("@uninterpreted") {
            pending = true;
            if let Some(sym) = extract_symbol(line) {
                pending_symbol = Some(sym);
            }
            continue;
        }
        if pending {
            // While a marker is pending, keep absorbing comment lines so
            // a `symbol="..."` on a continuation line is still captured.
            if is_comment_line(line) {
                if pending_symbol.is_none() {
                    if let Some(sym) = extract_symbol(line) {
                        pending_symbol = Some(sym);
                    }
                }
                continue;
            }
            if let Some(name) = signature_name(line) {
                out.push(UninterpretedEntry {
                    cryptol_fn: name,
                    symbol: pending_symbol.take(),
                });
            }
            // Either way, the marker is consumed by the first
            // non-comment line that follows it.
            pending = false;
            pending_symbol = None;
        }
    }
    out
}

/// Extract `symbol="..."` from a comment line, if present.
fn extract_symbol(line: &str) -> Option<String> {
    let idx = line.find("symbol")?;
    let rest = &line[idx + "symbol".len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Whether a (trimmed) line is purely a comment.
fn is_comment_line(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("//")
        || line.starts_with("/*")
        || line.starts_with('*')
        || line.ends_with("*/")
}

/// If `line` begins a Cryptol type signature (`ident : ...`), return the
/// identifier.
fn signature_name(line: &str) -> Option<String> {
    let colon = line.find(':')?;
    let ident = line[..colon].trim();
    if ident.is_empty() || ident.contains(char::is_whitespace) {
        return None;
    }
    let mut chars = ident.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if ident
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\'')
    {
        Some(ident.to_string())
    } else {
        None
    }
}

/// Build a SAWScript block binding each uninterpreted entry to its
/// Cryptol contract via `llvm_unsafe_assume_spec`.
///
/// Entries whose Cryptol signature cannot be parsed, or whose parameter /
/// return types are not expressible as a SAW contract, are skipped with
/// a warning comment in the snippet — the rest still emit.
pub fn emit_uninterpreted_block(
    entries: &[UninterpretedEntry],
    cryptol_spec: &Path,
) -> UninterpretedBlock {
    let mut block = UninterpretedBlock::default();
    for entry in entries {
        match cryptol_sig::parse_signature(cryptol_spec, &entry.cryptol_fn) {
            Some(sig) => emit_one(entry, &sig, &mut block),
            None => block.snippet.push_str(&format!(
                "// uninterpreted: could not parse Cryptol signature for `{}`; skipped\n",
                entry.cryptol_fn,
            )),
        }
    }
    block
}

/// Emit a single assume-spec block for one entry + parsed signature.
fn emit_one(entry: &UninterpretedEntry, sig: &CrySig, block: &mut UninterpretedBlock) {
    let id = sanitize_id(&entry.cryptol_fn);
    let symbol = entry.resolved_symbol();

    // Build per-parameter setup, collecting the Cryptol argument names
    // and the LLVM execute-func arguments.
    let mut setup = String::new();
    let mut cry_args: Vec<String> = Vec::new();
    let mut exec_args: Vec<String> = Vec::new();
    for (i, ty) in sig.params.iter().enumerate() {
        match param_lines(i, ty) {
            Some((lines, cry, exec)) => {
                setup.push_str(&lines);
                cry_args.push(cry);
                exec_args.push(exec);
            }
            None => {
                block.snippet.push_str(&format!(
                    "// uninterpreted: `{}` has unsupported parameter type {ty:?}; skipped\n",
                    entry.cryptol_fn,
                ));
                return;
            }
        }
    }

    let call = if cry_args.is_empty() {
        entry.cryptol_fn.clone()
    } else {
        format!("{} {}", entry.cryptol_fn, cry_args.join(" "))
    };

    // Return handling: scalars use `llvm_return`; an aggregate return
    // (`[N][M]`) is written through a hidden sret pointer.
    let (ret_setup, ret_post, sret_exec) = match return_lines(&sig.ret, &call) {
        Some(parts) => parts,
        None => {
            block.snippet.push_str(&format!(
                "// uninterpreted: `{}` has unsupported return type {:?}; skipped\n",
                entry.cryptol_fn, sig.ret,
            ));
            return;
        }
    };
    if let Some(sret) = sret_exec {
        exec_args.insert(0, sret);
    }

    let ov_name = format!("ov_uninterp_{id}");
    let mut header = format!(
        "// uninterpreted: {} (symbol: {symbol})\n",
        entry.cryptol_fn
    );
    if is_ct_compare_symbol(symbol) {
        header.push_str("// recognized constant-time comparison primitive\n");
    }
    block.snippet.push_str(&header);
    block
        .snippet
        .push_str(&format!("let {id}_uninterp_spec = do {{\n"));
    block.snippet.push_str(&ret_setup);
    block.snippet.push_str(&setup);
    block.snippet.push_str(&format!(
        "    llvm_execute_func [{}];\n",
        exec_args.join(", ")
    ));
    block.snippet.push_str(&ret_post);
    block.snippet.push_str("};\n");
    block.snippet.push_str(&format!(
        "{ov_name} <- llvm_unsafe_assume_spec m \"{symbol}\" {id}_uninterp_spec;\n\n"
    ));
    block.override_names.push(ov_name);
}

/// SAW setup for one parameter. Returns `(setup_lines, cryptol_arg,
/// execute_arg)` or `None` when the type cannot be expressed.
fn param_lines(i: usize, ty: &CryType) -> Option<(String, String, String)> {
    let name = format!("a{i}");
    match ty {
        CryType::Bit => Some((
            format!("    {name} <- llvm_fresh_var \"{name}\" (llvm_int 1);\n"),
            name.clone(),
            format!("llvm_term {name}"),
        )),
        CryType::Bitvector(bits) => Some((
            format!("    {name} <- llvm_fresh_var \"{name}\" (llvm_int {bits});\n"),
            name.clone(),
            format!("llvm_term {name}"),
        )),
        CryType::SeqBitvector { len, elem_bits } => {
            let ty_str = format!("llvm_array {len} (llvm_int {elem_bits})");
            let lines = format!(
                "    {name}_ptr <- llvm_alloc_readonly ({ty_str});\n\
                 \x20   {name} <- llvm_fresh_var \"{name}\" ({ty_str});\n\
                 \x20   llvm_points_to {name}_ptr (llvm_term {name});\n",
            );
            Some((lines, name.clone(), format!("{name}_ptr")))
        }
        CryType::Other(_) => None,
    }
}

/// SAW setup/postcondition for the return type. Returns `(pre_setup,
/// post, Some(sret_exec_arg))` for aggregate returns, or `(String::new,
/// llvm_return-line, None)` for scalars.
fn return_lines(ret: &CryType, call: &str) -> Option<(String, String, Option<String>)> {
    match ret {
        CryType::Bit | CryType::Bitvector(_) => Some((
            String::new(),
            format!("    llvm_return (llvm_term {{{{ {call} }}}});\n"),
            None,
        )),
        CryType::SeqBitvector { len, elem_bits } => {
            let ty_str = format!("llvm_array {len} (llvm_int {elem_bits})");
            let pre = format!("    result_ptr <- llvm_alloc ({ty_str});\n");
            let post = format!("    llvm_points_to result_ptr (llvm_term {{{{ {call} }}}});\n");
            Some((pre, post, Some("result_ptr".to_string())))
        }
        CryType::Other(_) => None,
    }
}

/// Reduce a Cryptol identifier to a SAW-safe variable stem.
fn sanitize_id(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{s}")
    } else {
        s
    }
}

#[cfg(test)]
#[path = "uninterpreted_tests.rs"]
mod tests;
