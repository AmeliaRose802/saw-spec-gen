//! Path-based pre-filter for clang JSON AST dumps.
//!
//! Strips top-level declarations originating in files outside the
//! user's project tree, before the rest of the parser ever sees them.
//!
//! Why
//! ===
//! Including a single STL header like `<string>` makes clang's
//! `-ast-dump=json` produce ~300 MB of JSON, almost all of it from
//! `xstring`, `xmemory`, `xutility`, and their templated
//! dependencies. The user's actual code is a few KB inside that.
//! Loading the full dump into memory both blows past
//! [`super::parse::MAX_AST_FILE_SIZE`] and is hugely wasteful: the
//! verifier doesn't reason about MSVC STL internals -- it treats any
//! call into them as an external havoc.
//!
//! Instead of maintaining an explicit allowlist of system-header
//! paths (which is brittle: MSVC includes move between toolchain
//! versions, GCC/clang each have their own layout, every platform
//! differs), this filter takes the opposite approach:
//!
//! > **Keep declarations whose source file lives under one of the
//! > user-provided `keep` directories. Drop everything else.**
//!
//! That's a single, mechanical rule. Anything the user wrote (the
//! `.cpp`, its sibling headers, any other folder they explicitly
//! pass) is preserved verbatim. Anything from elsewhere on disk
//! -- system headers, third-party libraries the user hasn't
//! whitelisted, the toolchain's own builtins -- is removed.
//!
//! Streaming
//! =========
//! The implementation parses the *whole* JSON into a `serde_json::Value`
//! first; it does not stream. This is intentional: clang's AST schema
//! has the file name elided on every node after the first one whose
//! `loc.file` matches the previous, so deciding whether to keep a
//! given top-level decl requires walking siblings in order. A
//! streaming parser would have to replicate the same state machine
//! anyway. The peak memory use (~5-10x the JSON file size) is a
//! one-shot cost paid by a separate binary; the filtered output that
//! flows into `gen-verify` is small enough to fit comfortably under
//! the normal 100 MB limit.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Statistics returned from [`filter_translation_unit_value`] for diagnostic
/// reporting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FilterStats {
    /// Top-level decls kept (their source file matched a `keep` prefix).
    pub kept: usize,
    /// Top-level decls dropped (their source file did not match).
    pub dropped: usize,
    /// Top-level decls with no detectable source file -- always kept,
    /// since dropping them would risk losing built-in / synthetic nodes
    /// like the implicit `TypedefDecl`s clang prepends to every TU.
    pub no_loc: usize,
}

/// Filter a parsed clang JSON AST in place, dropping top-level
/// `inner[]` entries whose source file isn't covered by any
/// `keep_paths` prefix.
///
/// Paths are compared case-insensitively after normalisation: each
/// node's `loc.file` is canonicalised (Windows backslashes -> forward
/// slashes, lowercased) and tested against each `keep_paths` entry
/// (similarly normalised). A prefix match wins.
///
/// Clang elides repeated `loc.file` entries when consecutive siblings
/// share a source file. This function tracks the most recently seen
/// file value as it iterates, so the decision is correct regardless
/// of elision.
pub fn filter_translation_unit_value(root: &mut Value, keep_paths: &[PathBuf]) -> FilterStats {
    let normalised_keep: Vec<String> = keep_paths.iter().map(|p| normalise_path(p)).collect();
    let mut stats = FilterStats::default();
    if !is_translation_unit(root) {
        // Single-decl dumps (e.g. `-ast-dump-filter=name`) don't have
        // an `inner` array to walk. Treat the root as kept.
        stats.kept = 1;
        return stats;
    }
    let Some(inner) = root.get_mut("inner").and_then(|i| i.as_array_mut()) else {
        return stats;
    };
    filter_decl_list(inner, &normalised_keep, &mut stats);
    stats
}

/// Walk a list of decls (the `inner[]` array of a TranslationUnitDecl
/// or a wrapper like LinkageSpecDecl / NamespaceDecl), keeping the
/// ones we want and dropping the rest in place.
///
/// Wrapper decls (`extern "C" { ... }`, `namespace std { ... }`) are
/// recursed into so that pure declarations of system functions like
/// `rand` / `printf` -- which clang nests several layers deep under
/// LinkageSpecDecl/NamespaceDecl living in vcruntime.h -- are
/// preserved even though their wrapper's `loc.file` is outside the
/// user's `keep` paths. After recursion, an empty wrapper is dropped.
fn filter_decl_list(inner: &mut Vec<Value>, normalised_keep: &[String], stats: &mut FilterStats) {
    let mut current_file: Option<String> = None;
    inner.retain_mut(|node| {
        let file = first_loc_file(node).map(|f| f.to_string());
        if let Some(f) = &file {
            current_file = Some(f.clone());
        }

        // Wrapper decls: recurse, then decide whether the (now
        // possibly trimmed) wrapper is still worth keeping.
        let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let is_wrapper = matches!(kind, "LinkageSpecDecl" | "NamespaceDecl");
        if is_wrapper {
            if let Some(child) = node.get_mut("inner").and_then(|i| i.as_array_mut()) {
                filter_decl_list(child, normalised_keep, stats);
                if child.is_empty() {
                    // Nothing survived the recursion. The wrapper
                    // itself doesn't carry a definition the verifier
                    // needs, so drop it.
                    return false;
                }
            }
            return true;
        }

        // Pure declarations (no body) are always cheap and gen-verify
        // needs them to emit external override specs for system
        // functions the user's code calls -- e.g. `int rand();` in
        // <cstdlib> or `printf` in <cstdio>. The path filter would
        // otherwise drop those declarations along with the rest of
        // the header, and SAW would later fail with "No implementation
        // or override found for pointer" at the call site.
        //
        // A FunctionDecl is a pure declaration iff none of its
        // `inner` children is a `CompoundStmt` (the function body
        // node). Pure declarations carry only the signature + maybe
        // a few `ParmVarDecl` children, so keeping them costs <1 KB
        // per node and is the right thing to do regardless of source.
        if is_pure_function_declaration(node) {
            stats.kept += 1;
            return true;
        }
        match current_file.as_deref() {
            Some(f) => {
                let canon = canonicalise(f);
                if normalised_keep
                    .iter()
                    .any(|kp| canon.starts_with(kp) || canon.contains(&format!("/{kp}")))
                {
                    stats.kept += 1;
                    true
                } else {
                    stats.dropped += 1;
                    false
                }
            }
            None => {
                stats.no_loc += 1;
                true
            }
        }
    });
}

/// True iff `root` looks like a clang `TranslationUnitDecl`.
fn is_translation_unit(root: &Value) -> bool {
    root.get("kind").and_then(|k| k.as_str()) == Some("TranslationUnitDecl")
}

/// True iff `node` is a `FunctionDecl` (or `CXXMethodDecl` etc.) that
/// carries no body. Clang represents the body as a child `CompoundStmt`
/// node in `inner[]`; a pure declaration has none.
///
/// Pure declarations are always kept by the path filter -- they are
/// cheap (signature + parameter decls only, no inlined template
/// instantiations) and gen-verify needs them to emit external override
/// specs for system functions like `printf` or `rand` that the user's
/// code calls.
fn is_pure_function_declaration(node: &Value) -> bool {
    let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let is_function = matches!(
        kind,
        "FunctionDecl"
            | "CXXMethodDecl"
            | "CXXConstructorDecl"
            | "CXXDestructorDecl"
            | "CXXConversionDecl"
    );
    if !is_function {
        return false;
    }
    let Some(inner) = node.get("inner").and_then(|i| i.as_array()) else {
        // No `inner` at all: it's a forward declaration.
        return true;
    };
    !inner
        .iter()
        .any(|c| c.get("kind").and_then(|k| k.as_str()) == Some("CompoundStmt"))
}

/// Find the first source-file string in any of the conventional spots
/// clang puts it on a top-level decl:
///   * `loc.file`
///   * `loc.spellingLoc.file`
///   * `loc.expansionLoc.file`
///   * `range.begin.file`
///   * `range.begin.spellingLoc.file`
///   * `range.begin.expansionLoc.file`
fn first_loc_file(node: &Value) -> Option<&str> {
    const CANDIDATES: &[&[&str]] = &[
        &["loc", "file"],
        &["loc", "spellingLoc", "file"],
        &["loc", "expansionLoc", "file"],
        &["range", "begin", "file"],
        &["range", "begin", "spellingLoc", "file"],
        &["range", "begin", "expansionLoc", "file"],
    ];
    for path in CANDIDATES {
        let mut cur = node;
        let mut ok = true;
        for key in *path {
            match cur.get(key) {
                Some(v) => cur = v,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if let Some(s) = cur.as_str() {
                return Some(s);
            }
        }
    }
    None
}

/// Lowercase + forward-slash normalisation so prefix comparisons are
/// platform-insensitive. We deliberately do *not* call
/// [`Path::canonicalize`] because the file may not exist on the
/// filtering machine (e.g. the AST was generated elsewhere).
///
/// We also strip the Windows extended-length / verbatim prefix
/// (`\\?\` or `\\?\UNC\`). `Path::canonicalize` produces verbatim paths
/// on Windows, but clang emits plain `C:\...` paths in the AST. Without
/// normalising the prefix the keep-dir (verbatim) never prefix-matches a
/// main-file decl (plain), so the target function gets filtered out.
fn canonicalise(p: &str) -> String {
    let slashed = p.replace('\\', "/").to_lowercase();
    if let Some(rest) = slashed.strip_prefix("//?/unc/") {
        format!("//{rest}")
    } else if let Some(rest) = slashed.strip_prefix("//?/") {
        rest.to_string()
    } else {
        slashed
    }
}

fn normalise_path(p: &Path) -> String {
    canonicalise(&p.display().to_string())
}

/// Streaming-ish entry point: read JSON from `input`, filter, write to
/// `output`. The `keep_paths` are checked against every top-level decl
/// as described on [`filter_translation_unit_value`].
///
/// Returns the [`FilterStats`] so callers can report what was dropped.
pub fn filter_ast_file(input: &Path, output: &Path, keep_paths: &[PathBuf]) -> Result<FilterStats> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("Failed to read {}", input.display()))?;
    let mut value: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", input.display()))?;
    let stats = filter_translation_unit_value(&mut value, keep_paths);
    let pretty = serde_json::to_string(&value)
        .with_context(|| "Failed to re-serialise filtered AST".to_string())?;
    std::fs::write(output, pretty)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(stats)
}

#[cfg(test)]
#[path = "path_filter_tests.rs"]
mod tests;
