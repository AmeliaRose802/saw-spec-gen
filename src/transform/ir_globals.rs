//! LLVM IR global-variable scanning.
//!
//! Extracted from `extern_override_scan` to keep that module under
//! the 500 non-whitespace-line limit.

use crate::constraints::{GlobalVarInfo, TypeInfo};
use std::collections::HashSet;

/// Mutable-global sets split by linkage visibility.
pub(crate) struct MutableGlobals {
    /// Every mutable global in the module (any linkage).
    pub all: HashSet<String>,
    /// Subset of `all` that uses external-ish linkage and is therefore
    /// reachable from other translation units. Globals with `internal`
    /// or `private` linkage are excluded.
    pub externally_visible: HashSet<String>,
}

/// Scan LLVM IR text for the set of **mutable** global variable symbols
/// (i.e. those declared `global` rather than `constant`).
///
/// Returns a [`MutableGlobals`] that partitions symbols by linkage
/// visibility. Globals with `internal` or `private` linkage cannot be
/// accessed from other translation units and are excluded from the
/// `externally_visible` subset.
///
/// Used by the bitcode-override emitter to decide which globals to
/// adversarially clobber in an extern override's post-state — a
/// `constant` global cannot be written to, so we must filter it out
/// to avoid SAW rejecting `llvm_points_to (llvm_global ...)` writes.
///
/// Matches lines like `@"NAME" = ... global TYPE ...` and
/// `@NAME = ... global TYPE ...`, returning bare symbols (no `@`,
/// no quotes).
pub(crate) fn scan_mutable_globals(ir: &str) -> MutableGlobals {
    let mut all = HashSet::new();
    let mut externally_visible = HashSet::new();
    for line in ir.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('@') {
            continue;
        }
        let Some(eq) = trimmed.find(" = ") else {
            continue;
        };
        let lhs = &trimmed[..eq];
        let rhs = &trimmed[eq + 3..];
        let needle = format!(" {rhs}");
        let g_idx = needle.find(" global ");
        let c_idx = needle.find(" constant ");
        let is_mutable = match (g_idx, c_idx) {
            (Some(g), Some(c)) => g < c,
            (Some(_), None) => true,
            _ => false,
        };
        if !is_mutable {
            continue;
        }
        let sym = lhs.trim_start_matches('@').trim();
        let sym = sym.trim_matches('"');
        if sym.is_empty() {
            continue;
        }
        // Skip exception-lower synthesised globals — they are internal
        // bookkeeping and must never be adversarially clobbered.
        if sym.starts_with("__exclow_") {
            continue;
        }
        let s = sym.to_string();
        all.insert(s.clone());
        let is_internal = rhs
            .split_whitespace()
            .any(|tok| tok == "internal" || tok == "private");
        if !is_internal {
            externally_visible.insert(s);
        }
    }
    MutableGlobals {
        all,
        externally_visible,
    }
}

/// Parse the LLVM scalar type token immediately after `global` and
/// return a [`TypeInfo`]. Returns `None` for non-scalar types (structs,
/// arrays, pointers) that we can't yet emit `llvm_fresh_var` for.
fn ir_type_to_type_info(tok: &str) -> Option<TypeInfo> {
    if let Some(rest) = tok.strip_prefix('i') {
        if let Ok(bits) = rest.parse::<u32>() {
            return Some(TypeInfo::SignedInt(bits));
        }
    }
    None
}

/// Discover mutable globals in the LLVM IR and return [`GlobalVarInfo`]
/// entries for any that are **not** already present in `ast_globals`
/// (keyed by mangled name). This fills the gap for function-local
/// `static` variables and compiler-generated globals that the clang
/// AST parser doesn't report.
///
/// Only scalar integer globals are returned — struct, array, and
/// pointer globals are skipped because we can't yet emit the correct
/// SAW allocation for them.
pub(crate) fn discover_ir_only_globals(
    ir: &str,
    ast_globals: &[GlobalVarInfo],
) -> Vec<GlobalVarInfo> {
    let known: HashSet<&str> = ast_globals
        .iter()
        .map(|g| g.mangled_name.as_str())
        .collect();
    let mut out = Vec::new();
    for line in ir.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('@') {
            continue;
        }
        let Some(eq) = trimmed.find(" = ") else {
            continue;
        };
        let lhs = &trimmed[..eq];
        let rhs = &trimmed[eq + 3..];
        // Must be `global`, not `constant`.
        let needle = format!(" {rhs}");
        let g_idx = needle.find(" global ");
        let c_idx = needle.find(" constant ");
        let is_mutable = match (g_idx, c_idx) {
            (Some(g), Some(c)) => g < c,
            (Some(_), None) => true,
            _ => false,
        };
        if !is_mutable {
            continue;
        }
        let sym = lhs.trim_start_matches('@').trim().trim_matches('"');
        if sym.is_empty() || known.contains(sym) {
            continue;
        }
        // Skip exception-lower synthesised globals — they are handled
        // separately by `eh_globals::inject_exclow_globals` and must
        // not end up in the adversarial-clobber set.
        if sym.starts_with("__exclow_") {
            continue;
        }
        // Find the type token right after `global `.
        let Some(global_pos) = rhs.find("global ") else {
            continue;
        };
        let after_global = &rhs[global_pos + 7..];
        let type_tok = after_global.split_whitespace().next().unwrap_or("");
        let Some(ty) = ir_type_to_type_info(type_tok) else {
            continue;
        };
        // Try to parse a constant integer initializer (e.g. `i32 7`).
        let init_value = after_global.split_whitespace().nth(1).and_then(|tok| {
            let tok = tok.trim_end_matches(',');
            tok.parse::<i64>().ok().map(|v| v.to_string())
        });
        out.push(GlobalVarInfo {
            name: sym.to_string(),
            mangled_name: sym.to_string(),
            ty,
            init_value,
            has_static_initializer: false,
        });
    }
    out
}

/// Scan LLVM IR text for the set of global symbols that are **defined
/// with a static initializer** in this module — i.e. `global`/`constant`
/// definitions whose linkage is *not* `external`/`extern_weak`. These
/// are exactly the globals for which SAW's `llvm_global_initializer`
/// works: it reads the compile-time initializer straight from the
/// module. External declarations (`@g = external global TY`, no
/// in-module initializer) are excluded because they have nothing to
/// seed from.
///
/// Used to mark [`GlobalVarInfo::has_static_initializer`] so the spec
/// emitter can seed aggregate globals (e.g. `static std::mutex g_mtx;`)
/// with their real static-initialized pre-state instead of leaving the
/// allocation uninitialized.
pub(crate) fn defined_global_symbols(ir: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in ir.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('@') {
            continue;
        }
        let Some(eq) = trimmed.find(" = ") else {
            continue;
        };
        let lhs = &trimmed[..eq];
        let rhs = &trimmed[eq + 3..];
        // Locate the ` global ` / ` constant ` keyword (whichever comes
        // first). The tokens before it are the linkage / attribute list.
        let needle = format!(" {rhs}");
        let kw_pos = match (needle.find(" global "), needle.find(" constant ")) {
            (Some(g), Some(c)) => g.min(c),
            (Some(g), None) => g,
            (None, Some(c)) => c,
            (None, None) => continue,
        };
        let prefix = &needle[..kw_pos];
        // `external`/`extern_weak` globals are declarations with no
        // in-module initializer, so `llvm_global_initializer` cannot
        // seed them — skip.
        if prefix
            .split_whitespace()
            .any(|t| t == "external" || t == "extern_weak")
        {
            continue;
        }
        let sym = lhs.trim_start_matches('@').trim().trim_matches('"');
        if sym.is_empty() {
            continue;
        }
        out.insert(sym.to_string());
    }
    out
}

/// Set [`GlobalVarInfo::has_static_initializer`] on every global that is
/// defined with a compile-time initializer in `ir` but for which we
/// couldn't render a scalar `init_value`. The spec emitter then seeds
/// such globals from their module static initializer
/// (`llvm_global_initializer`) instead of leaving the allocation
/// uninitialized — required for aggregate globals like
/// `static std::mutex g_mtx;`.
pub(crate) fn mark_static_initializers(globals: &mut [GlobalVarInfo], ir: &str) {
    let defined = defined_global_symbols(ir);
    for g in globals {
        if g.init_value.is_none() && defined.contains(&g.mangled_name) {
            g.has_static_initializer = true;
        }
    }
}

/// Match `store TY VALUE, ptr @NAME, ...` and return the bare global
/// symbol (no leading `@`, no surrounding quotes). Returns `None` for
/// stores to a value (`%reg`) or to a derived pointer (gep, bitcast),
/// because tracing those requires alias analysis we don't have.
///
/// The check is intentionally syntactic: we only recognise the form
/// `store ..., ptr @SYMBOL` (possibly with quoted symbol). Stores
/// through `getelementptr inbounds ... @GLOBAL` are not picked up
/// here — they'd need a full instruction parse to associate the
/// gep result name with its base global.
pub(crate) fn extract_store_global_target(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("store ") {
        return None;
    }
    let at = trimmed.find('@')?;
    let before = &trimmed[..at];
    if !before.contains(',') {
        return None;
    }
    let after_at = &trimmed[at + 1..];
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
mod tests {
    use super::*;

    #[test]
    fn defined_globals_include_internal_and_exclude_external() {
        let ir = "\
@g_mtx = internal global { i32, i32 } { i32 2, i32 0 }, align 8\n\
@user_state = global i32 0\n\
@extern_decl = external global i32\n\
@weak_decl = extern_weak global i32\n\
@vtbl = private unnamed_addr constant [2 x ptr] [ptr null, ptr null]\n";
        let defined = defined_global_symbols(ir);
        assert!(
            defined.contains("g_mtx"),
            "internal-with-init global missing"
        );
        assert!(
            defined.contains("user_state"),
            "plain defined global missing"
        );
        assert!(defined.contains("vtbl"), "private constant missing");
        assert!(
            !defined.contains("extern_decl"),
            "external declaration must be excluded (no in-module initializer)"
        );
        assert!(
            !defined.contains("weak_decl"),
            "extern_weak declaration must be excluded"
        );
    }

    #[test]
    fn defined_globals_ignore_non_global_lines() {
        let ir = "\
define i32 @f() {\n\
  ret i32 0\n\
}\n\
%struct.Foo = type { i32 }\n";
        assert!(defined_global_symbols(ir).is_empty());
    }

    #[test]
    fn mark_static_initializers_only_flags_defined_no_scalar_globals() {
        let ir = "\
@g_mtx = internal global { i32, i32 } { i32 2, i32 0 }, align 8\n\
@scalar = internal global i32 7\n\
@ext = external global { i32 }\n";
        let mut globals = vec![
            GlobalVarInfo {
                name: "g_mtx".into(),
                mangled_name: "g_mtx".into(),
                ty: TypeInfo::UnsignedInt(32),
                init_value: None,
                has_static_initializer: false,
            },
            // Has a scalar init_value already — must stay unflagged so the
            // emitter keeps rendering the concrete term.
            GlobalVarInfo {
                name: "scalar".into(),
                mangled_name: "scalar".into(),
                ty: TypeInfo::SignedInt(32),
                init_value: Some("7".into()),
                has_static_initializer: false,
            },
            // External declaration — no in-module initializer to seed.
            GlobalVarInfo {
                name: "ext".into(),
                mangled_name: "ext".into(),
                ty: TypeInfo::UnsignedInt(32),
                init_value: None,
                has_static_initializer: false,
            },
        ];
        mark_static_initializers(&mut globals, ir);
        assert!(globals[0].has_static_initializer, "g_mtx should be flagged");
        assert!(
            !globals[1].has_static_initializer,
            "scalar-init global must not be flagged"
        );
        assert!(
            !globals[2].has_static_initializer,
            "external declaration must not be flagged"
        );
    }
}
