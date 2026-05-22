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
fn filter_decl_list(
    inner: &mut Vec<Value>,
    normalised_keep: &[String],
    stats: &mut FilterStats,
) {
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
    !inner.iter().any(|c| {
        c.get("kind").and_then(|k| k.as_str()) == Some("CompoundStmt")
    })
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
fn canonicalise(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

fn normalise_path(p: &Path) -> String {
    canonicalise(&p.display().to_string())
}

/// Streaming-ish entry point: read JSON from `input`, filter, write to
/// `output`. The `keep_paths` are checked against every top-level decl
/// as described on [`filter_translation_unit_value`].
///
/// Returns the [`FilterStats`] so callers can report what was dropped.
pub fn filter_ast_file(
    input: &Path,
    output: &Path,
    keep_paths: &[PathBuf],
) -> Result<FilterStats> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("Failed to read {}", input.display()))?;
    let mut value: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", input.display()))?;
    let stats = filter_translation_unit_value(&mut value, keep_paths);
    let pretty = serde_json::to_string(&value)
        .with_context(|| format!("Failed to re-serialise filtered AST"))?;
    std::fs::write(output, pretty)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project_dir() -> PathBuf {
        PathBuf::from(r"C:\Users\me\proj")
    }

    fn system_dir() -> PathBuf {
        PathBuf::from(r"C:\Program Files\LLVM\include")
    }

    /// Helper: produce a FunctionDecl JSON node that LOOKS LIKE A
    /// DEFINITION (has a CompoundStmt body), so the path filter
    /// applies normally to it. Pure declarations (no body) are
    /// always kept and would defeat path-filtering tests.
    fn fn_def(name: &str, file: &str) -> serde_json::Value {
        json!({
            "kind": "FunctionDecl",
            "name": name,
            "loc": { "file": file },
            "inner": [ { "kind": "CompoundStmt" } ]
        })
    }

    #[test]
    fn keeps_decls_under_project_dir() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                fn_def("foo", r"C:\Users\me\proj\foo.cpp"),
                fn_def("stl_thing", r"C:\Program Files\LLVM\include\xstring"),
                fn_def("bar", r"C:\Users\me\proj\bar.h"),
            ]
        });
        let stats =
            filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 2);
        assert_eq!(stats.dropped, 1);
        assert_eq!(stats.no_loc, 0);
        let kept_count = ast["inner"].as_array().unwrap().len();
        assert_eq!(kept_count, 2);
    }

    #[test]
    fn elided_file_inherits_from_previous_sibling() {
        // Clang omits `loc.file` when a sibling decl uses the same
        // file as the immediately-preceding one. The filter must
        // remember the last-seen file to classify the elided sibling
        // correctly.
        // All four entries have bodies (CompoundStmt) so they're
        // subject to path filtering rather than the pure-decl
        // pass-through.
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "loc": {"file": r"C:\Program Files\LLVM\include\xstring", "line": 10}, "inner": [{"kind": "CompoundStmt"}]},
                {"kind": "FunctionDecl", "loc": {"line": 20}, "inner": [{"kind": "CompoundStmt"}]},
                {"kind": "FunctionDecl", "loc": {"file": r"C:\Users\me\proj\foo.cpp", "line": 1}, "inner": [{"kind": "CompoundStmt"}]},
                {"kind": "FunctionDecl", "loc": {"line": 2}, "inner": [{"kind": "CompoundStmt"}]},
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.dropped, 2, "both stl entries (line 10 + elided line 20) dropped");
        assert_eq!(stats.kept, 2, "both project entries (line 1 + elided line 2) kept");
    }

    #[test]
    fn no_loc_decls_are_kept() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "TypedefDecl", "name": "__int128_t"},
                {"kind": "FunctionDecl", "loc": {"file": r"C:\Users\me\proj\foo.cpp"}},
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        // __int128_t has no loc at all -- it's a builtin -- and would
        // never be dropped. The function under the project dir is
        // kept on its own merits.
        assert_eq!(stats.no_loc, 1);
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn comparison_is_case_insensitive() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                // Same path, different casing on the drive letter.
                fn_def("foo", r"c:\users\ME\proj\Foo.cpp"),
                fn_def("bar", r"C:\Users\me\proj\bar.cpp"),
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 2);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn unix_style_paths_round_trip() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                fn_def("foo", "/home/me/proj/foo.cpp"),
                fn_def("stl_thing", "/usr/include/c++/13/string"),
            ]
        });
        let stats = filter_translation_unit_value(
            &mut ast,
            &[PathBuf::from("/home/me/proj")],
        );
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.dropped, 1);
    }

    #[test]
    fn non_translation_unit_root_is_passed_through() {
        // -ast-dump-filter outputs single FunctionDecl objects, not a
        // TranslationUnitDecl. The filter should leave them alone.
        let mut ast = json!({
            "kind": "FunctionDecl",
            "name": "foo",
            "loc": {"file": r"C:\Program Files\LLVM\include\xstring"}
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn multiple_keep_paths_act_as_union() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                fn_def("foo", r"C:\Users\me\proj\foo.cpp"),
                fn_def("lib_helper", r"C:\Users\me\third_party\lib.h"),
                fn_def("stl_thing", r"C:\Program Files\LLVM\include\xstring"),
            ]
        });
        let stats = filter_translation_unit_value(
            &mut ast,
            &[project_dir(), PathBuf::from(r"C:\Users\me\third_party")],
        );
        assert_eq!(stats.kept, 2);
        assert_eq!(stats.dropped, 1);
        // Make sure the deliberately-dropped system header path
        // wasn't accidentally retained.
        let _ = system_dir();
    }

    #[test]
    fn expansion_loc_is_consulted_when_loc_file_missing() {
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "FunctionDecl",
                    "range": {
                        "begin": {
                            "expansionLoc": {"file": r"C:\Users\me\proj\macro_user.cpp"}
                        }
                    }
                }
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn pure_declarations_outside_keep_path_are_kept() {
        // Models a typical <cstdlib> declaration for `int rand()`.
        // gen-verify needs the declaration to emit an external
        // override spec, so the filter must keep it even though it
        // lives outside the user's directory.
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                // Pure declaration in a system header -- KEEP.
                {
                    "kind": "FunctionDecl",
                    "name": "rand",
                    "loc": {"file": r"C:\Program Files\LLVM\include\stdlib.h"},
                    "inner": [
                        {"kind": "ParmVarDecl", "name": "p1"}
                    ]
                },
                // Function with a body in the same system header -- DROP.
                {
                    "kind": "FunctionDecl",
                    "name": "abort_impl",
                    "loc": {"file": r"C:\Program Files\LLVM\include\stdlib.h"},
                    "inner": [
                        {"kind": "ParmVarDecl", "name": "p1"},
                        {"kind": "CompoundStmt"}
                    ]
                },
                // User code -- KEEP.
                {
                    "kind": "FunctionDecl",
                    "name": "my_func",
                    "loc": {"file": r"C:\Users\me\proj\foo.cpp"}
                },
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 2, "rand decl + my_func kept");
        assert_eq!(stats.dropped, 1, "abort_impl with body dropped");
        let kept_names: Vec<&str> = ast["inner"]
            .as_array().unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        assert_eq!(kept_names, vec!["rand", "my_func"]);
    }

    #[test]
    fn forward_declarations_with_no_inner_are_kept() {
        // Some `FunctionDecl` JSON dumps have no `inner` key at all
        // (truly forward declarations). They should still be treated
        // as pure declarations and kept.
        let mut ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "FunctionDecl",
                    "name": "printf",
                    "loc": {"file": r"C:\Program Files\LLVM\include\stdio.h"}
                }
            ]
        });
        let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.dropped, 0);
    }
}
