//! Resolve opaque `llvm_alias "Name"` references in generated SAW specs
//! against either the LLVM IR struct-size table or an LLVM
//! `dereferenceable(N)` annotation extracted from the function signature.
//!
//! Background: clang ASTs reference C++ classes by their short, unqualified
//! name (`HttpRequest`), but LLVM IR emitted by MSVC fully-qualifies the
//! struct (`%"struct.Microsoft::Azure::...::HttpRequest" = type { ... }`).
//! A spec that emits `llvm_alias "HttpRequest"` therefore fails to load
//! because SAW can't find a symbol matching that short name.  This module
//! provides the substitution logic to convert those aliases into either a
//! resolved byte-array (`llvm_array N (llvm_int 8)`) or — when the bitcode
//! itself wasn't supplied via `--llvm-ir` but the function signature had a
//! `dereferenceable(N)` attribute — a size-only byte-array using N.

use std::collections::HashMap;

#[doc(hidden)]
// Re-export the internal IR resolution function for the post-processing
// pass in `spec_rewrite`.  We keep `resolve_via_ir` private because it's
// an implementation detail of the public `resolve_saw_type` API; this
// wrapper lets a sibling module reuse the matching logic without
// duplicating it.
pub fn resolve_via_ir_pub(saw_type: &str, ir_sizes: &HashMap<String, usize>) -> Option<String> {
    resolve_via_ir(saw_type, ir_sizes)
}

/// Returns true when `saw_type` is an `llvm_alias` reference that gen-verify
/// should try to resolve through the LLVM IR struct table.
pub fn needs_resolution(saw_type: &str) -> bool {
    saw_type.starts_with("llvm_alias \"")
}

/// Look up an opaque `llvm_alias "X"` against the LLVM IR struct-size map
/// and, when a unique match is found, return the byte-array substitute.
///
/// Matching strategy:
///   1. Exact match on the alias's inner name.
///   2. Strip `struct.` / `class.` / `union.` prefix, then match any IR
///      name whose own simple-name suffix equals the AST short name
///      (covers MSVC fully-qualified names like `struct.Foo::Bar::Baz`).
///   3. Template-argument short-name match: compares `std::optional<T>`
///      against `std::optional<ns::T>` by normalising every template
///      argument to its last `::` component — handles the Itanium ABI
///      case where the IR emits `class.std::optional<protocol::T>` while
///      the AST qualType stores `std::optional<T>` without a namespace.
///
/// Ambiguous matches (two candidates at any step) return None so the
/// caller can warn rather than silently pick the wrong layout.
fn resolve_via_ir(saw_type: &str, ir_sizes: &HashMap<String, usize>) -> Option<String> {
    if ir_sizes.is_empty() {
        return None;
    }
    let prefix = "llvm_alias \"";
    if !saw_type.starts_with(prefix) || !saw_type.ends_with('"') {
        return None;
    }
    let name = &saw_type[prefix.len()..saw_type.len() - 1];

    // 1. Exact match against the IR table.
    if let Some(&n) = ir_sizes.get(name) {
        return Some(format!("llvm_array {n} (llvm_int 8)"));
    }

    // 2. Suffix match on the simple C++ name.
    fn strip_prefix(s: &str) -> &str {
        s.strip_prefix("struct.")
            .or_else(|| s.strip_prefix("class."))
            .or_else(|| s.strip_prefix("union."))
            .unwrap_or(s)
    }
    let simple = strip_prefix(name);
    let needle = format!("::{simple}");
    let candidates: Vec<usize> = ir_sizes
        .iter()
        .filter_map(|(k, v)| {
            let k_simple = strip_prefix(k);
            if k_simple == simple || k_simple.ends_with(&needle) {
                Some(*v)
            } else {
                None
            }
        })
        .collect();
    if candidates.len() == 1 {
        return Some(format!("llvm_array {} (llvm_int 8)", candidates[0]));
    }

    // 3. Template-argument short-name match.
    //    Normalise all namespace prefixes in template arguments so that
    //    `std::optional<EnrollmentKey>` matches
    //    `std::optional<protocol::EnrollmentKey>` in the IR.
    if name.contains('<') {
        let name_norm = normalize_template_args(name);
        let matching_sizes: Vec<usize> = ir_sizes
            .iter()
            .filter_map(|(k, v)| {
                let k_simple = strip_prefix(k);
                if k_simple.contains('<') && normalize_template_args(k_simple) == name_norm {
                    Some(*v)
                } else {
                    None
                }
            })
            .collect();
        if matching_sizes.len() == 1 {
            return Some(format!("llvm_array {} (llvm_int 8)", matching_sizes[0]));
        }
    }

    None
}

/// Return the last `::` component of a potentially namespace-qualified name.
fn last_component(s: &str) -> &str {
    s.rfind("::").map_or(s, |pos| &s[pos + 2..])
}

/// Normalise a template instantiation string by reducing each argument to
/// its last `::` component.
///
/// Example: `std::optional<protocol::EnrollmentKey>` →
///          `std::optional<EnrollmentKey>`
///
/// Nested templates are handled recursively: each argument may itself
/// contain `<…>` and will be normalised in the same way.
fn normalize_template_args(s: &str) -> String {
    let Some(open) = s.find('<') else {
        return s.to_string();
    };
    let base = &s[..open];
    // Find the matching closing `>` for the outermost `<`.
    let args_and_rest = &s[open + 1..];
    let close = find_closing_angle(args_and_rest);
    let args = &args_and_rest[..close];
    // Split args at depth-0 commas.  The `args` slice contains only the
    // content between the outermost `<` and its matching `>`, so every `>`
    // here is a nested closing bracket; depth should not underflow on
    // well-formed input.  Use explicit guard to avoid panic on malformed input.
    let mut out_args: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in args.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '>' => {} // extra `>` in malformed input — ignore
            ',' if depth == 0 => {
                out_args.push(normalize_one_arg(args[start..i].trim()));
                start = i + 1;
            }
            _ => {}
        }
    }
    out_args.push(normalize_one_arg(args[start..].trim()));
    format!("{}<{}>", base, out_args.join(", "))
}

/// Normalise a single template argument: if it contains `<`, recurse;
/// otherwise strip its namespace prefix.
fn normalize_one_arg(arg: &str) -> String {
    if arg.contains('<') {
        normalize_template_args(arg)
    } else {
        last_component(arg).to_string()
    }
}

/// Return the index of the `>` that closes the outermost `<` whose
/// contents are `s` (i.e. the `<` has already been consumed).
/// Returns `s.len()` if no matching `>` is found.
///
/// `depth` counts unmatched `<` seen so far; the bare `'>'` arm can only
/// be reached when `depth > 0` (the `if depth == 0` guard returns first),
/// so `depth -= 1` here is always safe and cannot underflow.
fn find_closing_angle(s: &str) -> usize {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            // depth == 0: this `>` closes the outermost `<` — return its index.
            '>' if depth == 0 => return i,
            // depth > 0: this `>` closes a nested `<`; safe to subtract.
            '>' => depth -= 1,
            _ => {}
        }
    }
    s.len()
}

/// Resolve `saw_type` against the IR struct table, falling back to a
/// `dereferenceable(N)` annotation size when the IR table has no entry.
///
/// The IR-table path returns the canonical byte size derived from the
/// actual loaded module — preferred whenever available.  The
/// `deref_fallback` is used only as a last resort and is correct for
/// override specs (where SAW just needs an allocation large enough to
/// satisfy the LLVM signature).
pub fn resolve_saw_type(
    saw_type: &str,
    ir_sizes: &HashMap<String, usize>,
    deref_fallback: Option<usize>,
) -> Option<String> {
    if let Some(r) = resolve_via_ir(saw_type, ir_sizes) {
        return Some(r);
    }
    if needs_resolution(saw_type) {
        if let Some(n) = deref_fallback {
            return Some(format!("llvm_array {n} (llvm_int 8)"));
        }
    }
    None
}

/// Resolve all `llvm_alias "X"` parameter & return types on `spec`
/// against `ir_sizes`, mutating `spec` in place.  Logs each successful
/// resolution and emits a warning for any types left unresolved.
///
/// `ir_was_supplied` controls the warning text: when the caller did not
/// pass `--llvm-ir`, the warning suggests doing so; otherwise it points
/// the user at AST coverage / IR coverage as the likely culprit.
pub fn resolve_spec_aliases(
    spec: &mut crate::constraints::SpecConstraint,
    ir_sizes: &HashMap<String, usize>,
    ir_was_supplied: bool,
) {
    let mut unresolved: Vec<String> = Vec::new();
    for p in &mut spec.params {
        if let Some(replacement) = resolve_saw_type(&p.saw_type, ir_sizes, p.dereferenceable_size) {
            eprintln!(
                "  resolved param '{}' type {} → {}",
                p.name, p.saw_type, replacement
            );
            p.saw_type = replacement;
        } else if needs_resolution(&p.saw_type) {
            unresolved.push(format!("param '{}' type {}", p.name, p.saw_type));
        }
    }
    if let Some(replacement) = resolve_saw_type(&spec.return_constraint.saw_type, ir_sizes, None) {
        eprintln!(
            "  resolved return type {} → {}",
            spec.return_constraint.saw_type, replacement
        );
        spec.return_constraint.saw_type = replacement;
    } else if needs_resolution(&spec.return_constraint.saw_type) {
        unresolved.push(format!("return type {}", spec.return_constraint.saw_type));
    }
    if unresolved.is_empty() {
        return;
    }
    if ir_was_supplied {
        eprintln!(
            "warning: {} unresolved opaque struct type(s) — generated spec may fail to load:",
            unresolved.len()
        );
        for u in &unresolved {
            eprintln!("  - {u}");
        }
        eprintln!("  hint: ensure the .ll file contains a matching `%struct.X = type {{...}}`,");
        eprintln!("        or pass an additional --ast file with the struct's full definition.");
    } else {
        eprintln!(
            "warning: {} opaque struct type(s) in target spec; pass --llvm-ir <path>",
            unresolved.len()
        );
        eprintln!("        to resolve them against the bitcode's struct table:");
        for u in &unresolved {
            eprintln!("  - {u}");
        }
    }
}

/// Silently resolve `llvm_alias` references in a spec's params and
/// return type. Unlike [`resolve_spec_aliases`], this does not log
/// warnings — used for bulk override-spec resolution in `gen_verify`.
pub fn resolve_spec_types_quiet(
    spec: &mut crate::constraints::SpecConstraint,
    ir_sizes: &HashMap<String, usize>,
) {
    for p in &mut spec.params {
        if let Some(r) = resolve_saw_type(&p.saw_type, ir_sizes, p.dereferenceable_size) {
            p.saw_type = r;
        }
    }
    if let Some(r) = resolve_saw_type(&spec.return_constraint.saw_type, ir_sizes, None) {
        spec.return_constraint.saw_type = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_exact_match() {
        let mut m = HashMap::new();
        m.insert("struct.PacketHeader".to_string(), 24usize);
        let out = resolve_saw_type("llvm_alias \"struct.PacketHeader\"", &m, None);
        assert_eq!(out.as_deref(), Some("llvm_array 24 (llvm_int 8)"));
    }

    #[test]
    fn resolve_suffix_match_namespaced() {
        let mut m = HashMap::new();
        m.insert(
            "struct.Microsoft::Azure::Host::HttpRequest".to_string(),
            112usize,
        );
        let out = resolve_saw_type("llvm_alias \"struct.HttpRequest\"", &m, None);
        assert_eq!(out.as_deref(), Some("llvm_array 112 (llvm_int 8)"));
    }

    #[test]
    fn resolve_ambiguous_returns_none() {
        let mut m = HashMap::new();
        m.insert("struct.A::Thing".to_string(), 8usize);
        m.insert("struct.B::Thing".to_string(), 16usize);
        let out = resolve_saw_type("llvm_alias \"struct.Thing\"", &m, None);
        assert!(out.is_none());
    }

    #[test]
    fn resolve_skips_non_alias_types() {
        let mut m = HashMap::new();
        m.insert("struct.X".to_string(), 4usize);
        assert!(resolve_saw_type("llvm_int 32", &m, None).is_none());
        assert!(resolve_saw_type("llvm_array 8 (llvm_int 8)", &m, None).is_none());
    }

    #[test]
    fn resolve_empty_map_is_noop() {
        let m: HashMap<String, usize> = HashMap::new();
        assert!(resolve_saw_type("llvm_alias \"struct.X\"", &m, None).is_none());
    }

    #[test]
    fn resolve_uses_dereferenceable_fallback_when_ir_table_misses() {
        let m: HashMap<String, usize> = HashMap::new();
        let out = resolve_saw_type("llvm_alias \"HttpRequest\"", &m, Some(112));
        assert_eq!(out.as_deref(), Some("llvm_array 112 (llvm_int 8)"));
    }

    #[test]
    fn resolve_prefers_ir_table_over_dereferenceable_fallback() {
        let mut m = HashMap::new();
        m.insert("struct.HttpRequest".to_string(), 96usize);
        let out = resolve_saw_type("llvm_alias \"HttpRequest\"", &m, Some(999));
        assert_eq!(out.as_deref(), Some("llvm_array 96 (llvm_int 8)"));
    }

    #[test]
    fn resolve_dereferenceable_fallback_ignored_for_non_alias() {
        let m: HashMap<String, usize> = HashMap::new();
        let out = resolve_saw_type("llvm_int 32", &m, Some(112));
        assert!(out.is_none());
    }

    #[test]
    fn resolve_template_arg_namespace_mismatch_itanium() {
        // Fix 3: Itanium ABI emits `class.std::optional<protocol::EnrollmentKey>`
        // but the AST qualType is `std::optional<EnrollmentKey>`.
        // Step 3 of resolve_via_ir must match after normalising template args.
        let mut m = HashMap::new();
        m.insert(
            "class.std::optional<protocol::EnrollmentKey>".to_string(),
            16usize,
        );
        let out = resolve_saw_type("llvm_alias \"std::optional<EnrollmentKey>\"", &m, None);
        assert_eq!(
            out.as_deref(),
            Some("llvm_array 16 (llvm_int 8)"),
            "template-arg namespace mismatch must still resolve"
        );
    }

    #[test]
    fn resolve_template_already_matches_without_normalise() {
        // Exact match on `std::optional<EnrollmentKey>` (MSVC ABI — no
        // namespace in template arg) — Step 1 handles this, Step 3 must
        // not interfere.
        let mut m = HashMap::new();
        m.insert("std::optional<EnrollmentKey>".to_string(), 16usize);
        let out = resolve_saw_type("llvm_alias \"std::optional<EnrollmentKey>\"", &m, None);
        assert_eq!(out.as_deref(), Some("llvm_array 16 (llvm_int 8)"));
    }

    #[test]
    fn normalize_template_args_strips_single_namespace() {
        // Unit test for the helper — not exported but tested via resolve.
        // Ensure `std::optional<ns::T>` → `std::optional<T>`.
        let normalised = normalize_template_args("std::optional<protocol::EnrollmentKey>");
        assert_eq!(normalised, "std::optional<EnrollmentKey>");
    }

    #[test]
    fn normalize_template_args_handles_nested_templates() {
        // `std::pair<ns::A, std::optional<ns::B>>` →
        // `std::pair<A, std::optional<B>>`
        let normalised = normalize_template_args("std::pair<ns::A, std::optional<ns::B>>");
        assert_eq!(normalised, "std::pair<A, std::optional<B>>");
    }

    #[test]
    fn resolve_template_arg_ambiguous_returns_none() {
        // Two IR entries that both normalise to the same template — must
        // return None (ambiguous), not pick one arbitrarily.
        let mut m = HashMap::new();
        m.insert("class.std::optional<ns1::T>".to_string(), 8usize);
        m.insert("class.std::optional<ns2::T>".to_string(), 16usize);
        let out = resolve_saw_type("llvm_alias \"std::optional<T>\"", &m, None);
        assert!(out.is_none(), "ambiguous template match must return None");
    }
}
