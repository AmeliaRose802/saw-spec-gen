//! Post-processing pass that rewrites unresolved `llvm_alias "X"`
//! references in generated SAW specs to concrete SAW types.
//!
//! Background: emitters like `generate_havoc_spec` and the vtable stub
//! setups go through `constraints::type_to_saw`, which produces
//! `llvm_alias "ShortName"` for any C++ struct / opaque type without a
//! known byte size.  SAW's `llvm_load_module` resolves aliases against
//! the bitcode's `%struct.X = type { ... }` table — and MSVC-clang
//! fully namespace-qualifies every C++ struct
//! (`%"struct.Microsoft::Azure::…::HttpRequest"`).  A spec that
//! references the short name therefore fails to load with "alias not
//! found" before any verification work begins.
//!
//! This module provides a final-pass rewrite that scans every `.saw`
//! file under the output directory and replaces each unresolvable
//! `llvm_alias "X"` with either `llvm_array N (llvm_int 8)` (struct /
//! opaque) or `llvm_int <bits>` (enums in the ABI), using the
//! [`AliasFallbacks`] map produced by `alias_fallbacks::collect_type_sizes`.
//!
//! This is sound for override specs because SAW only needs the
//! allocation size, not the field layout, when treating memory as a
//! symbolic blob.

pub use crate::alias_fallbacks::{collect_type_sizes, AliasFallbacks};
use crate::type_resolve::resolve_via_ir_pub;
use std::collections::{HashMap, HashSet};

/// Rewrite every `llvm_alias "X"` occurrence in `text` that's not
/// resolvable through the IR struct table with the appropriate fallback:
///   - enums (`fallbacks.enum_bits` hit) → `llvm_int <bits>`
///   - everything else with a known byte size → `llvm_array N (llvm_int 8)`
///
/// Returns the rewritten text plus a set of alias names that remained
/// unresolved (no entry in any map).  Aliases that *do* resolve via the
/// IR struct table are also rewritten to the byte-array form — this
/// matches the behaviour of `resolve_saw_type` and produces consistent
/// output regardless of whether resolution happened at spec build time
/// or via this post-processing pass.
pub fn rewrite_unresolved_aliases(
    text: &str,
    ir_sizes: &HashMap<String, usize>,
    fallbacks: &AliasFallbacks,
) -> (String, HashSet<String>) {
    let pattern = "llvm_alias \"";
    let mut unresolved: HashSet<String> = HashSet::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(pattern) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + pattern.len()..];
        // Find the closing `"`.  SAW alias names never contain a double
        // quote, so a simple scan is enough.
        if let Some(close) = after.find('"') {
            let name = &after[..close];
            let alias_text = format!("llvm_alias \"{name}\"");
            // Exact IR struct-name match: SAW's `llvm_alias` resolves
            // these natively, and the original alias preserves struct
            // shape (fields + layout). Flattening to a byte array here
            // would break callers that write `llvm_struct_value` into the
            // allocation (e.g. constructor overrides) — SAW rejects the
            // store with "types not memory-compatible: [N x i8] vs { ... }".
            // Only fall through to the byte-array rewrite when the alias
            // needs *renaming* (suffix matching on a short C++ name → the
            // IR's fully-qualified `struct.Foo::Bar::Baz`).
            if ir_sizes.contains_key(name) {
                out.push_str(&alias_text);
            } else if let Some(r) = resolve_via_ir_pub(&alias_text, ir_sizes) {
                out.push_str(&r);
            } else if let Some(bits) = fallbacks.enum_bits.get(name) {
                // Enum types lower to a single LLVM integer in the ABI
                // (most C++ enums are `i32`).  Emit `llvm_int N` so SAW
                // accepts the typed store the bitcode performs against
                // the allocation; a byte array would fail the
                // memory-compatibility check.
                out.push_str(&format!("llvm_int {bits}"));
            } else if let Some(n) = fallbacks.bytes.get(name) {
                out.push_str(&format!("llvm_array {n} (llvm_int 8)"));
            } else {
                // Leave alias intact and remember it for the warning log.
                out.push_str(&alias_text);
                unresolved.insert(name.to_string());
            }
            rest = &after[close + 1..];
        } else {
            // Malformed input — emit the rest verbatim and stop.
            out.push_str(&rest[pos..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    (out, unresolved)
}

/// Apply `rewrite_unresolved_aliases` to every `.saw` file under `dir`
/// (recursively).  Returns the union of unresolved alias names across
/// all files, suitable for emitting a single warning.
pub fn rewrite_specs_dir(
    dir: &std::path::Path,
    ir_sizes: &HashMap<String, usize>,
    fallbacks: &AliasFallbacks,
) -> std::io::Result<HashSet<String>> {
    let mut unresolved: HashSet<String> = HashSet::new();
    walk_saw_files(dir, &mut |path| -> std::io::Result<()> {
        let text = std::fs::read_to_string(path)?;
        let (rewritten, file_unresolved) =
            rewrite_unresolved_aliases(&text, ir_sizes, fallbacks);
        if rewritten != text {
            std::fs::write(path, rewritten)?;
        }
        unresolved.extend(file_unresolved);
        Ok(())
    })?;
    Ok(unresolved)
}

/// Walk `dir` recursively and invoke `f` on every `.saw` file found.
fn walk_saw_files<F>(dir: &std::path::Path, f: &mut F) -> std::io::Result<()>
where
    F: FnMut(&std::path::Path) -> std::io::Result<()>,
{
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_saw_files(&path, f)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("saw") {
            f(&path)?;
        }
    }
    Ok(())
}

/// High-level entry point: rewrite every `.saw` file under `dir`,
/// emitting human-readable diagnostics on stderr.  Used by gen-verify
/// at the end of the pipeline so the generated specs reference only
/// concrete `llvm_array N (llvm_int 8)` allocations for types whose
/// LLVM IR struct name doesn't match the alias the AST produced.
pub fn apply_alias_rewrites(
    output: &std::path::Path,
    ir_sizes: &HashMap<String, usize>,
    fallbacks: &AliasFallbacks,
) {
    match rewrite_specs_dir(output, ir_sizes, fallbacks) {
        Ok(unresolved) if !unresolved.is_empty() => {
            eprintln!(
                "warning: {} alias type(s) could not be resolved to a byte size;",
                unresolved.len(),
            );
            eprintln!(
                "         SAW may fail to load if the bitcode lacks a matching struct:"
            );
            let mut names: Vec<_> = unresolved.into_iter().collect();
            names.sort();
            for n in names {
                eprintln!("  - llvm_alias \"{n}\"");
            }
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "warning: post-processing pass for llvm_alias resolution failed: {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallbacks_with_bytes(pairs: &[(&str, usize)]) -> AliasFallbacks {
        let mut f = AliasFallbacks::default();
        for (k, v) in pairs {
            f.bytes.insert((*k).into(), *v);
        }
        f
    }

    #[test]
    fn rewrite_replaces_unresolved_alias_with_extra_size() {
        let ir: HashMap<String, usize> = HashMap::new();
        let fb = fallbacks_with_bytes(&[("std::string", 32)]);
        let (out, unresolved) = rewrite_unresolved_aliases(
            "  s_ptr <- llvm_alloc (llvm_alias \"std::string\");\n",
            &ir,
            &fb,
        );
        assert!(out.contains("llvm_array 32 (llvm_int 8)"), "got: {out}");
        assert!(!out.contains("llvm_alias \"std::string\""));
        assert!(unresolved.is_empty());
    }

    #[test]
    fn rewrite_keeps_alias_when_no_size_known() {
        let ir: HashMap<String, usize> = HashMap::new();
        let fb = AliasFallbacks::default();
        let (out, unresolved) = rewrite_unresolved_aliases(
            "  p <- llvm_alloc (llvm_alias \"Mystery\");\n",
            &ir,
            &fb,
        );
        assert!(out.contains("llvm_alias \"Mystery\""));
        assert!(unresolved.contains("Mystery"));
    }

    #[test]
    fn rewrite_rewrites_struct_alias_to_byte_array() {
        let ir: HashMap<String, usize> = HashMap::new();
        let fb = fallbacks_with_bytes(&[("HttpRequest", 112)]);
        let input = "a <- llvm_alloc (llvm_alias \"HttpRequest\");";
        let (out, unresolved) = rewrite_unresolved_aliases(input, &ir, &fb);
        assert!(out.contains("llvm_array 112 (llvm_int 8)"), "got: {out}");
        assert!(unresolved.is_empty());
    }

    #[test]
    fn rewrite_uses_llvm_int_for_enum_fallback() {
        // Enums lower to a single LLVM integer in the ABI — emit
        // `llvm_int N` so SAW accepts the typed store the bitcode
        // performs against the allocation. A byte array would trip
        // the memory-compatibility check.
        let ir: HashMap<String, usize> = HashMap::new();
        let mut fb = AliasFallbacks::default();
        fb.enum_bits.insert("LatchResult".into(), 32);
        fb.enum_bits.insert("SessionState".into(), 32);
        let input = "a <- llvm_alloc (llvm_alias \"LatchResult\"); \
                     b <- llvm_alloc (llvm_alias \"SessionState\");";
        let (out, unresolved) = rewrite_unresolved_aliases(input, &ir, &fb);
        assert!(out.contains("llvm_int 32"), "got: {out}");
        assert!(!out.contains("llvm_alias \"LatchResult\""));
        assert!(!out.contains("llvm_alias \"SessionState\""));
        assert!(
            !out.contains("llvm_array 4 (llvm_int 8)"),
            "enums must NOT fall back to byte arrays: {out}"
        );
        assert!(unresolved.is_empty());
    }

    #[test]
    fn rewrite_prefers_enum_fallback_over_bytes() {
        // If both an enum bit-width and a byte size are recorded for
        // the same name (e.g. AST has the enum definition AND a forward
        // declaration with dereferenceable(4)), the enum form wins.
        let ir: HashMap<String, usize> = HashMap::new();
        let mut fb = fallbacks_with_bytes(&[("LatchResult", 4)]);
        fb.enum_bits.insert("LatchResult".into(), 32);
        let (out, _) = rewrite_unresolved_aliases(
            "x <- llvm_alloc (llvm_alias \"LatchResult\");",
            &ir,
            &fb,
        );
        assert!(out.contains("llvm_int 32"), "got: {out}");
        assert!(!out.contains("llvm_array"), "got: {out}");
    }

    #[test]
    fn rewrite_preserves_exact_ir_match() {
        // When an alias exactly matches an IR struct name, SAW can
        // resolve it natively (the IR has the field layout). Preserve
        // the alias so callers that write `llvm_struct_value` into the
        // allocation (e.g. constructor overrides) don't get a
        // memory-incompatible byte array.
        let mut ir: HashMap<String, usize> = HashMap::new();
        ir.insert("struct.Foo".into(), 64);
        let fb = fallbacks_with_bytes(&[("struct.Foo", 999)]);
        let (out, _) = rewrite_unresolved_aliases(
            "x <- llvm_alloc (llvm_alias \"struct.Foo\");",
            &ir,
            &fb,
        );
        assert!(
            out.contains("llvm_alias \"struct.Foo\""),
            "exact IR match should be preserved as alias; got: {out}"
        );
        assert!(
            !out.contains("llvm_array"),
            "no byte-array rewrite expected for exact IR match; got: {out}"
        );
    }

    #[test]
    fn rewrite_handles_text_with_no_aliases() {
        let ir: HashMap<String, usize> = HashMap::new();
        let fb = AliasFallbacks::default();
        let input = "// no aliases\nlet x = 1;\n";
        let (out, unresolved) = rewrite_unresolved_aliases(input, &ir, &fb);
        assert_eq!(out, input);
        assert!(unresolved.is_empty());
    }
}
