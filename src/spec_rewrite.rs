//! Post-processing pass that rewrites unresolved `llvm_alias "X"`
//! references in generated SAW specs to sized byte arrays.
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
//! This module provides a final-pass rewrite that:
//!
//! 1. Collects every byte-size hint we know about (struct field layout,
//!    enum discriminant width, well-known STL/Win32 sizes via
//!    `clang_ast::lookup_known_type_size`) into a `name → size_bytes`
//!    map.
//! 2. Scans every `.saw` file under the output directory and replaces
//!    each unresolvable `llvm_alias "X"` with `llvm_array N (llvm_int 8)`
//!    using the map.
//!
//! This is sound for override specs because SAW only needs the
//! allocation size, not the field layout, when treating memory as a
//! symbolic blob.

use crate::constraints::{FunctionInfo, TypeInfo};
use crate::type_resolve::resolve_via_ir_pub;
use std::collections::{HashMap, HashSet};

/// Walk a tree of `TypeInfo` values and record `name → size_bytes` for any
/// struct / opaque / enum that carries a size we can use as a fallback
/// when an `llvm_alias` reference can't be resolved against the LLVM IR
/// struct table.
///
/// Sources contributing sizes:
///   - `TypeInfo::Struct { size_bytes: Some(n), .. }` — clang AST struct
///     definitions where every field has a known C++ size (computed via
///     `compute_struct_size_from_fields`).
///   - `TypeInfo::Opaque { size_bytes: n>0, .. }` — well-known STL /
///     platform types looked up via `clang_ast::lookup_known_type_size`
///     (`std::string` → 32, `std::vector<…>` → 24, `GUID` → 16, …).
///   - `TypeInfo::Enum { discriminant_bits, .. }` — enums whose Itanium /
///     MSVC ABI byte size is `ceil(bits/8)`.  Most C++ enums lower to
///     `i32` (`SessionState`, `LatchResult`) and therefore 4 bytes.
///
/// The map keys are the *short* C++ names that `type_to_saw` emits as
/// `llvm_alias "X"`; the rewrite below looks them up verbatim.
/// Pointer / array / scalar variants don't contribute.
pub fn collect_type_sizes(funcs: &[FunctionInfo]) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for f in funcs {
        for p in &f.params {
            walk_type_for_sizes(&p.ty, &mut out);
        }
        walk_type_for_sizes(&f.return_type, &mut out);
        for g in &f.referenced_globals {
            walk_type_for_sizes(&g.ty, &mut out);
        }
    }
    out
}

/// Recursive helper for `collect_type_sizes`.
fn walk_type_for_sizes(ty: &TypeInfo, out: &mut HashMap<String, usize>) {
    match ty {
        TypeInfo::Struct {
            name,
            size_bytes: Some(n),
            fields,
        } => {
            insert_size(out, name, *n);
            for (_, fty) in fields {
                walk_type_for_sizes(fty, out);
            }
        }
        TypeInfo::Struct { name, fields, .. } => {
            // Try lookup_known_type_size for templated STL names that may
            // not have field-derived size (e.g. `std::tuple<…>`).
            if let Some(n) = crate::clang_ast::lookup_known_type_size(name) {
                insert_size(out, name, n);
            }
            for (_, fty) in fields {
                walk_type_for_sizes(fty, out);
            }
        }
        TypeInfo::Opaque {
            name,
            size_bytes: n,
        } if *n > 0 => {
            insert_size(out, name, *n);
        }
        TypeInfo::Opaque { name, .. } => {
            if let Some(n) = crate::clang_ast::lookup_known_type_size(name) {
                insert_size(out, name, n);
            }
        }
        TypeInfo::Enum {
            name,
            discriminant_bits,
            ..
        } => {
            let bytes = (((*discriminant_bits) as usize) + 7) / 8;
            // Treat 0-bit enums as 1-byte (defensive); most C++ enums are 32-bit.
            insert_size(out, name, bytes.max(1));
        }
        TypeInfo::Pointer(inner) => walk_type_for_sizes(inner, out),
        TypeInfo::Option(inner) => walk_type_for_sizes(inner, out),
        TypeInfo::Result(ok, err) => {
            walk_type_for_sizes(ok, out);
            walk_type_for_sizes(err, out);
        }
        _ => {}
    }
}

/// Insert only when missing or when the new value is larger.  Picking the
/// larger size is safer for override allocations — under-allocating risks
/// SAW reading past the end during havoc; over-allocating is harmless
/// because the override treats the buffer as a symbolic blob.
fn insert_size(map: &mut HashMap<String, usize>, name: &str, size: usize) {
    let entry = map.entry(name.to_string()).or_insert(size);
    if size > *entry {
        *entry = size;
    }
}

/// Rewrite every `llvm_alias "X"` occurrence in `text` that's not
/// resolvable through the IR struct table with `llvm_array N (llvm_int 8)`,
/// using `extra_sizes` as the lookup.
///
/// Returns the rewritten text plus a set of alias names that remained
/// unresolved (no entry in either map).  Aliases that *do* resolve via
/// the IR struct table are also rewritten — this matches the behaviour of
/// `resolve_saw_type` and produces consistent output regardless of
/// whether resolution happened at spec build time or via this
/// post-processing pass.
pub fn rewrite_unresolved_aliases(
    text: &str,
    ir_sizes: &HashMap<String, usize>,
    extra_sizes: &HashMap<String, usize>,
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
            } else if let Some(n) = extra_sizes.get(name) {
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
    extra_sizes: &HashMap<String, usize>,
) -> std::io::Result<HashSet<String>> {
    let mut unresolved: HashSet<String> = HashSet::new();
    walk_saw_files(dir, &mut |path| -> std::io::Result<()> {
        let text = std::fs::read_to_string(path)?;
        let (rewritten, file_unresolved) =
            rewrite_unresolved_aliases(&text, ir_sizes, extra_sizes);
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
    extra_sizes: &HashMap<String, usize>,
) {
    match rewrite_specs_dir(output, ir_sizes, extra_sizes) {
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
    use crate::constraints::{Mutability, Nullability, ParamInfo};

    fn empty_func() -> FunctionInfo {
        FunctionInfo {
            name: "f".into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    fn param(name: &str, ty: TypeInfo) -> ParamInfo {
        ParamInfo {
            name: name.into(),
            ty,
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }
    }

    #[test]
    fn rewrite_replaces_unresolved_alias_with_extra_size() {
        let ir: HashMap<String, usize> = HashMap::new();
        let mut extra: HashMap<String, usize> = HashMap::new();
        extra.insert("std::string".into(), 32);
        let (out, unresolved) = rewrite_unresolved_aliases(
            "  s_ptr <- llvm_alloc (llvm_alias \"std::string\");\n",
            &ir,
            &extra,
        );
        assert!(out.contains("llvm_array 32 (llvm_int 8)"), "got: {out}");
        assert!(!out.contains("llvm_alias \"std::string\""));
        assert!(unresolved.is_empty());
    }

    #[test]
    fn rewrite_keeps_alias_when_no_size_known() {
        let ir: HashMap<String, usize> = HashMap::new();
        let extra: HashMap<String, usize> = HashMap::new();
        let (out, unresolved) = rewrite_unresolved_aliases(
            "  p <- llvm_alloc (llvm_alias \"Mystery\");\n",
            &ir,
            &extra,
        );
        assert!(out.contains("llvm_alias \"Mystery\""));
        assert!(unresolved.contains("Mystery"));
    }

    #[test]
    fn rewrite_rewrites_multiple_aliases() {
        let ir: HashMap<String, usize> = HashMap::new();
        let mut extra: HashMap<String, usize> = HashMap::new();
        extra.insert("HttpRequest".into(), 112);
        extra.insert("SessionState".into(), 4);
        let input = "a <- llvm_alloc (llvm_alias \"HttpRequest\"); \
                     b <- llvm_alloc (llvm_alias \"SessionState\");";
        let (out, unresolved) = rewrite_unresolved_aliases(input, &ir, &extra);
        assert!(out.contains("llvm_array 112 (llvm_int 8)"));
        assert!(out.contains("llvm_array 4 (llvm_int 8)"));
        assert!(unresolved.is_empty());
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
        let mut extra: HashMap<String, usize> = HashMap::new();
        extra.insert("struct.Foo".into(), 999);
        let (out, _) = rewrite_unresolved_aliases(
            "x <- llvm_alloc (llvm_alias \"struct.Foo\");",
            &ir,
            &extra,
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
        let extra: HashMap<String, usize> = HashMap::new();
        let input = "// no aliases\nlet x = 1;\n";
        let (out, unresolved) = rewrite_unresolved_aliases(input, &ir, &extra);
        assert_eq!(out, input);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn collect_sizes_picks_up_struct_with_size() {
        let mut f = empty_func();
        f.params.push(param(
            "p",
            TypeInfo::Pointer(Box::new(TypeInfo::Struct {
                name: "MyStruct".into(),
                size_bytes: Some(24),
                fields: vec![],
            })),
        ));
        let sizes = collect_type_sizes(&[f]);
        assert_eq!(sizes.get("MyStruct").copied(), Some(24));
    }

    #[test]
    fn collect_sizes_picks_up_enum_discriminant_bits() {
        let mut f = empty_func();
        f.params.push(param(
            "s",
            TypeInfo::Enum {
                name: "SessionState".into(),
                variants: vec!["A".into(), "B".into()],
                discriminant_bits: 32,
            },
        ));
        let sizes = collect_type_sizes(&[f]);
        assert_eq!(sizes.get("SessionState").copied(), Some(4));
    }

    #[test]
    fn collect_sizes_picks_up_opaque_with_known_stl_name() {
        let mut f = empty_func();
        f.params.push(param(
            "s",
            TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                name: "std::string".into(),
                size_bytes: 0,
            })),
        ));
        let sizes = collect_type_sizes(&[f]);
        assert_eq!(sizes.get("std::string").copied(), Some(32));
    }
}
