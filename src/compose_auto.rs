//! Automatic compositional-contract discovery (assume-guarantee).
//!
//! A C++ function `A` defined in one translation unit and *called* from
//! another appears in the caller's bitcode only as a body-less
//! `declare`. By default saw-spec-gen models such a callee as an
//! unspecified extern (fresh symbolic return + havoc frame), which is
//! sound but maximally imprecise — any caller property depending on
//! `A`'s real behaviour cannot be proven.
//!
//! This module discovers, with **no configuration**, which declare-only
//! callees have a proven Cryptol contract and returns them as
//! [`UninterpretedEntry`] bindings so the caller can be verified
//! compositionally (`llvm_unsafe_assume_spec`). The mapping uses the
//! same `<name>` / `<name>_spec` convention the rest of the tool already
//! follows (`add_one` ↔ `add_one_spec`).
//!
//! Soundness is standard non-circular assume-guarantee: each leaf `A` is
//! discharged separately (the pipeline verifies every Cryptol top-level
//! def against its implementation), and the caller merely *assumes* the
//! contract while proving `B`.

use crate::parsers::cryptol_sig;
use crate::transform::extern_override_scan::{scan, BrokenReason};
use crate::uninterpreted::UninterpretedEntry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Discover compositional contracts for declare-only cross-TU callees
/// reachable from `target_symbol`.
///
/// For each declare-only extern callee, the implementation symbol is
/// mapped to a C++ source name via `symbol_to_source` (falling back to
/// the symbol itself, which is correct for `extern "C"` callees). If the
/// Cryptol spec defines a top-level function named `<name>` or
/// `<name>_spec` with a parseable signature, an assumed-contract entry
/// is emitted binding that symbol to the contract.
pub fn auto_compose_entries(
    cryptol_spec: &Path,
    ir_text: &str,
    target_symbol: &str,
    symbol_to_source: &HashMap<String, String>,
) -> Vec<UninterpretedEntry> {
    if ir_text.is_empty() || target_symbol.is_empty() {
        return Vec::new();
    }
    let cry_text = std::fs::read_to_string(cryptol_spec).unwrap_or_default();
    let cry_fns = top_level_sig_names(&cry_text);
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for t in scan(ir_text, target_symbol) {
        if t.reason != BrokenReason::DeclareOnly || !seen.insert(t.symbol.clone()) {
            continue;
        }
        let base = symbol_to_source
            .get(&t.symbol)
            .cloned()
            .unwrap_or_else(|| t.symbol.clone());
        let candidates = [
            format!("{base}_spec"),
            base.clone(),
            format!("{}_spec", t.symbol),
            t.symbol.clone(),
        ];
        for cand in candidates {
            if cry_fns.contains(&cand)
                && cryptol_sig::parse_signature(cryptol_spec, &cand).is_some()
            {
                out.push(UninterpretedEntry {
                    cryptol_fn: cand,
                    symbol: Some(t.symbol.clone()),
                });
                break;
            }
        }
    }
    out
}

/// Names of Cryptol top-level values carrying a type signature
/// (`name : type` at column 0).
fn top_level_sig_names(cry_text: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for line in cry_text.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim();
        let mut chars = name.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if first_ok && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            names.insert(name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_names_ignores_indented_and_comments() {
        let text = "double_it_spec : [32] -> [32]\ndouble_it_spec x = x * 2\n    inner : [8]\n// note: not a sig\nuse_spec : [32] -> [32]\n";
        let names = top_level_sig_names(text);
        assert!(names.contains("double_it_spec"));
        assert!(names.contains("use_spec"));
        assert!(!names.contains("inner")); // indented
        assert_eq!(names.len(), 2);
    }
}
