//! Bitcode-driven scan for functions SAW cannot symbolically execute.
//!
//! When `verify.saw` runs, SAW expects every function reachable from the
//! verification target to either (a) be loadable from the bitcode and
//! tractable for symbolic execution, or (b) have an
//! `llvm_unsafe_assume_spec` override registered before `llvm_verify`.
//!
//! Two classes of function trip this:
//!
//!   1. **Declare-only** symbols — externs from other TUs (libc,
//!      ucrt, OS APIs) that SAW sees as `[external function]` GlobalAllocs
//!      and aborts with `internal: error: in <name>` when entered.
//!   2. **Variadic-intrinsic users** — functions whose body calls
//!      `llvm.va_start.p0`, `llvm.va_end.p0`, etc. Crucible-LLVM has no
//!      shim for these, and even when SAW applies a havoc override on
//!      the intrinsic itself, downstream `va_arg` loads explode because
//!      the va_list memory was never materialized. The only sound option
//!      is to override the variadic function as a whole.
//!
//! The clang-AST path filter aggressively drops declarations under
//! system include paths to keep the spec graph bounded. That means
//! [`crate::gen_verify::run`]'s AST-derived `external_calls` set has
//! a blind spot exactly where it matters most: things like `printf`
//! whose declaration lives in `<cstdio>` get filtered out even though
//! the user's code calls them.
//!
//! This module operates **purely on the LLVM IR text** that gets fed to
//! SAW. Every overrideable target is grounded in a symbol that actually
//! appears in the bitcode, which is the only ground truth SAW will see.

use std::collections::{HashMap, HashSet, VecDeque};

use super::ir_globals::extract_store_global_target;
pub(crate) use super::ir_globals::{scan_mutable_globals, MutableGlobals};

/// A function in the bitcode that the emitter should wrap in a
/// signature-based `llvm_unsafe_assume_spec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideTarget {
    /// LLVM symbol as it appears in the bitcode (already mangled).
    pub symbol: String,
    /// Raw LLVM type tokens for each declared (fixed) parameter, in
    /// declaration order. For a variadic function this excludes the
    /// `...` tail — SAW's matcher ignores the vararg portion at call
    /// sites and only checks the fixed prefix.
    pub fixed_param_ir_types: Vec<String>,
    /// Raw LLVM type token for the return slot. `"void"` means the
    /// emitted spec must omit `llvm_return`.
    pub return_ir_type: String,
    /// Whether the declaration ends in `, ...)`. Carried for diagnostics
    /// only; the override emission strategy is identical for variadic
    /// and non-variadic targets — both use the fixed prefix only.
    pub is_variadic: bool,
    /// Why this target landed in the broken set. Surfaced in the
    /// generated SAW comment so a reader can tell at a glance whether
    /// the symbol is a true extern or a varargs body.
    pub reason: BrokenReason,
    /// Mutable globals this target may write, computed via transitive
    /// `store ..., ptr @G` reachability from defined bodies reachable
    /// from `symbol`, plus a conservative over-approximation for any
    /// opaque (DeclareOnly) callees encountered in the walk.
    ///
    /// **Defined bodies** (UsesVarargsIntrinsic): the BFS walks every
    /// defined callee transitively reachable in the same module and
    /// unions the `store ..., ptr @G` targets it finds.
    ///
    /// **Opaque callees** (DeclareOnly, including forward-declared
    /// functions from other project TUs): the BFS conservatively adds
    /// every mutable global with external linkage, because the opaque
    /// body could `extern` the symbol in its own TU and write it.
    /// Globals with `internal` or `private` linkage are excluded
    /// since other TUs cannot access them.
    ///
    /// The bitcode-override emitter uses this set to decide which
    /// `llvm_points_to (llvm_global ...)` post-conditions to attach,
    /// instead of conservatively clobbering every mutable global the
    /// IR declares.
    pub globals_written: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokenReason {
    /// The bitcode contains only a `declare` line, no body.
    DeclareOnly,
    /// The body uses `llvm.va_start.p0`, `llvm.va_end.p0`,
    /// `llvm.va_copy`, or `llvm.va_arg`.
    UsesVarargsIntrinsic,
    /// The symbol matched the `saw_spec_gen-jpp` STL functional
    /// override registry — Crucible can technically simulate the
    /// body but tends to panic deep inside libstdc++ allocator
    /// helpers (`Cannot mux LLVM values`). Force-havoc instead.
    StlOverride,
}

/// Per-function parsed snapshot used internally by [`scan`].
struct IrFunc {
    name: String,
    fixed_params: Vec<String>,
    return_ty: String,
    is_variadic: bool,
    is_define: bool,
    body_calls: Vec<String>,
    body_uses_va_intrinsic: bool,
    /// Bare global symbol names (no leading `@`, no quotes) that this
    /// function body directly stores to via `store ..., ptr @NAME`.
    /// Indirect stores through pointer arguments or loaded pointers
    /// are not tracked — they require alias analysis we don't have.
    body_globals_stored: Vec<String>,
}

/// Compute the set of bitcode symbols that need overrides for a verify
/// run targeting `target_symbol`.
///
/// The reachability walk follows `call`/`invoke` edges starting from
/// the target. Only callees actually present in the bitcode get
/// considered, which automatically excludes inline-eliminated helpers
/// and tail-called wrappers from the override set.
pub fn scan(ir: &str, target_symbol: &str) -> Vec<OverrideTarget> {
    let funcs = parse_functions(ir);
    let by_name: HashMap<&str, &IrFunc> = funcs.iter().map(|f| (f.name.as_str(), f)).collect();
    let mg = scan_mutable_globals(ir);

    // Reachability BFS from the target.
    let mut reachable: HashSet<String> = HashSet::new();
    let mut worklist: Vec<String> = vec![target_symbol.to_string()];
    while let Some(s) = worklist.pop() {
        if !reachable.insert(s.clone()) {
            continue;
        }
        if let Some(f) = by_name.get(s.as_str()) {
            for callee in &f.body_calls {
                if !reachable.contains(callee.as_str()) {
                    worklist.push(callee.clone());
                }
            }
        }
    }

    // Filter to the broken subset.
    let mut out = Vec::new();
    for f in &funcs {
        if !reachable.contains(&f.name) {
            continue;
        }
        if f.name == target_symbol {
            continue; // never override the function under verification
        }
        if f.name.starts_with("llvm.") {
            // Intrinsics are handled by Crucible-LLVM natively or are
            // unfixable at the override layer (see `verify_probe_leaves`
            // experiment: overriding `llvm.va_start.p0` still leaves the
            // caller body broken). Skip them here; the broken caller
            // (e.g. `printf`) will be the override target instead.
            continue;
        }
        let reason = if !f.is_define {
            BrokenReason::DeclareOnly
        } else if f.body_uses_va_intrinsic {
            BrokenReason::UsesVarargsIntrinsic
        } else if crate::emit::saw_emit::stl_overrides::matches(&f.name) {
            BrokenReason::StlOverride
        } else {
            continue;
        };
        let globals_written = collect_globals_written_from(&f.name, &by_name, &mg);
        out.push(OverrideTarget {
            symbol: f.name.clone(),
            fixed_param_ir_types: f.fixed_params.clone(),
            return_ir_type: f.return_ty.clone(),
            is_variadic: f.is_variadic,
            reason,
            globals_written,
        });
    }
    out.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    out
}

/// Walk every defined body reachable from `start` (through `call`/
/// `invoke` edges), unioning the bare global symbol names that those
/// bodies directly `store` into. When the walk reaches a DeclareOnly
/// callee, conservatively union **all externally-visible** mutable
/// globals — the opaque body could be a forward-declared function
/// from another project TU that `extern`s any such symbol and writes
/// it. Globals with `internal`/`private` linkage are excluded since
/// other TUs cannot reference them.
fn collect_globals_written_from(
    start: &str,
    by_name: &HashMap<&str, &IrFunc>,
    mg: &MutableGlobals,
) -> Vec<String> {
    let mut written: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut worklist: VecDeque<String> = VecDeque::new();
    worklist.push_back(start.to_string());
    while let Some(s) = worklist.pop_front() {
        if !visited.insert(s.clone()) {
            continue;
        }
        let Some(f) = by_name.get(s.as_str()) else {
            // Not even declared in the module — skip.
            continue;
        };
        if !f.is_define {
            // LLVM intrinsics are compiler-implemented primitives,
            // not real external functions — they cannot access user
            // globals.
            if s.starts_with("llvm.") {
                continue;
            }
            // Opaque callee — could be a forward-declared function
            // from another project TU that writes any externally-
            // visible global.
            written.extend(mg.externally_visible.iter().cloned());
            continue;
        }
        for g in &f.body_globals_stored {
            if mg.all.contains(g.as_str()) {
                written.insert(g.clone());
            }
        }
        for callee in &f.body_calls {
            if !visited.contains(callee.as_str()) {
                worklist.push_back(callee.clone());
            }
        }
    }
    let mut out: Vec<String> = written.into_iter().collect();
    out.sort();
    out
}

fn parse_functions(ir: &str) -> Vec<IrFunc> {
    let lines: Vec<&str> = ir.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let is_define = trimmed.starts_with("define ");
        let is_declare = trimmed.starts_with("declare ");
        if !is_define && !is_declare {
            i += 1;
            continue;
        }
        let Some(parsed) = parse_signature(trimmed) else {
            i += 1;
            continue;
        };
        let (name, fixed_params, return_ty, is_variadic) = parsed;
        let mut body_calls: Vec<String> = Vec::new();
        let mut body_uses_va = false;
        let mut body_globals_stored: Vec<String> = Vec::new();
        if is_define {
            // Body spans from the opening `{` (often on the define line
            // itself) through the matching `}`.
            let mut j = i + 1;
            while j < lines.len() {
                let bl = lines[j];
                let bt = bl.trim_start();
                if bt.starts_with('}') {
                    break;
                }
                if let Some(callee) = extract_call_target(bt) {
                    if callee.starts_with("llvm.va_start")
                        || callee.starts_with("llvm.va_end")
                        || callee.starts_with("llvm.va_copy")
                        || callee == "llvm.va_arg"
                    {
                        body_uses_va = true;
                    }
                    body_calls.push(callee);
                }
                if let Some(g) = extract_store_global_target(bt) {
                    body_globals_stored.push(g);
                }
                j += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
        out.push(IrFunc {
            name,
            fixed_params,
            return_ty,
            is_variadic,
            is_define,
            body_calls,
            body_uses_va_intrinsic: body_uses_va,
            body_globals_stored,
        });
    }
    out
}

/// Returns `(name, fixed_param_types, return_type, is_variadic)`.
fn parse_signature(line: &str) -> Option<(String, Vec<String>, String, bool)> {
    let at = line.find('@')?;
    let open = line[at..].find('(').map(|p| p + at)?;
    let close = find_matching_paren(line, open)?;

    let name_raw = line[at + 1..open].trim();
    let name = if let Some(stripped) = name_raw.strip_prefix('"') {
        stripped.trim_end_matches('"').to_string()
    } else {
        name_raw.to_string()
    };

    let params_str = &line[open + 1..close];
    let mut params: Vec<String> = Vec::new();
    let mut is_variadic = false;
    for p in split_top_level_commas(params_str) {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if p == "..." {
            is_variadic = true;
            continue;
        }
        // Take the leading type token (first whitespace-delimited word,
        // possibly followed by `*` or `<...>` for aggregates).
        let ty = leading_type_token(p);
        params.push(ty);
    }

    // Return type is whatever sits between the leading keyword
    // (`define`/`declare`) and the `@name`. Skip linkage / visibility /
    // calling-convention / parameter-attribute keywords.
    let prefix = &line[..at];
    let ret_ty = extract_return_type_token(prefix);

    Some((name, params, ret_ty, is_variadic))
}

fn find_matching_paren(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'(' | b'<' | b'[' | b'{' => depth += 1,
            b')' | b'>' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn leading_type_token(p: &str) -> String {
    // Take everything up to the first space NOT inside `<...>`/`{...}`.
    let mut depth = 0i32;
    let mut end = p.len();
    for (i, b) in p.bytes().enumerate() {
        match b {
            b'<' | b'{' | b'[' | b'(' => depth += 1,
            b'>' | b'}' | b']' | b')' => depth -= 1,
            b' ' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    p[..end].to_string()
}

fn extract_return_type_token(prefix: &str) -> String {
    // Walk tokens right-to-left; the return type is the last token that
    // isn't a known keyword.  We use a bracket-aware splitter so that
    // compound LLVM types such as `[16 x i8]` or `{ i64, ptr }` are
    // treated as a single token rather than being torn apart by the
    // space between `[16`, `x`, and `i8]`.
    let known = [
        "define",
        "declare",
        "dso_local",
        "dso_preemptable",
        "local_unnamed_addr",
        "unnamed_addr",
        "internal",
        "linkonce",
        "linkonce_odr",
        "weak",
        "weak_odr",
        "external",
        "private",
        "available_externally",
        "common",
        "hidden",
        "protected",
        "default",
        "nounwind",
        "noundef",
        "readonly",
        "readnone",
        "argmemonly",
        "norecurse",
        "thread_local",
        "preemption_specifier",
        "ccc",
        "fastcc",
        "coldcc",
        "x86_stdcallcc",
        "tailcc",
        "swifttailcc",
        "musttailcc",
        "zeroext",
        "signext",
    ];
    let tokens = split_bracket_tokens(prefix);
    for t in tokens.iter().rev() {
        // Calling conventions can also appear as `cc 10` etc. Skip
        // bare numbers.
        if t.parse::<i64>().is_ok() {
            continue;
        }
        if known.contains(&t.as_str()) {
            continue;
        }
        if t.starts_with("alignstack")
            || t.starts_with("dereferenceable")
            || t.starts_with("range(")
        {
            continue;
        }
        return t.clone();
    }
    "void".to_string()
}

/// Split `s` on whitespace keeping bracket-enclosed spans as one token (depth only, no pair-check).
fn split_bracket_tokens(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for b in s.bytes() {
        match b {
            b'[' | b'(' | b'{' | b'<' => {
                depth += 1;
                cur.push(b as char);
            }
            b']' | b')' | b'}' | b'>' => {
                depth -= 1;
                cur.push(b as char);
            }
            b' ' | b'\t' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(b as char),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn extract_call_target(line: &str) -> Option<String> {
    // Match both `call ... @name(...)` and `invoke ... @name(...)`.
    // Variadic call syntax contains a parenthesized type signature
    // before the `@`:  `call i32 (ptr, ...) @printf(ptr, i32)`. So we
    // can't anchor on `(` — the callee marker is always the FIRST `@`
    // after the keyword (LLVM IR type tokens never contain `@`).
    let kw_pos = line.find("call ").or_else(|| line.find("invoke "))?;
    let after = &line[kw_pos..];
    let at = after.find('@')?;
    let after_at = &after[at + 1..];
    let name = if let Some(rest) = after_at.strip_prefix('"') {
        let end = rest.find('"')?;
        rest[..end].to_string()
    } else {
        let bytes = after_at.as_bytes();
        let mut end = 0;
        while end < bytes.len() {
            let c = bytes[end];
            if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$') {
                break;
            }
            end += 1;
        }
        if end == 0 {
            return None;
        }
        after_at[..end].to_string()
    };
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
#[path = "extern_override_scan_tests.rs"]
mod tests;
