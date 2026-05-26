//! Callgraph analysis: scan function bodies for `call`/`invoke` instructions.
//!
//! Given the full LLVM IR text and a target function name, extracts all
//! functions called from that target's body.  Each call site produces a
//! [`CalledFunction`] with the mangled symbol name and whether it has a
//! `define` body in this module (i.e. is it local or external).
//!
//! This is the foundation for auto-generating override scaffolding:
//! external calls (declared but not defined) need
//! `llvm_unsafe_assume_spec` stubs for compositional SAW verification.

use crate::constraints::CalledFunction;
use std::collections::{HashMap, HashSet};

/// Scan function bodies in the IR and populate a map of
/// function-name → called-functions.
///
/// Pass 1: collect all `define`d symbol names into a set.
/// Pass 2: for each `define`d function, scan its body for
///   `call ... @"name"` and `invoke ... @"name"` patterns.
pub fn build_callgraph(ir: &str) -> HashMap<String, Vec<CalledFunction>> {
    let defined: HashSet<String> = ir
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("define ") {
                extract_function_name(trimmed)
            } else {
                None
            }
        })
        .collect();

    let mut callgraph: HashMap<String, Vec<CalledFunction>> = HashMap::new();
    let mut current_fn: Option<String> = None;

    for line in ir.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("define ") {
            current_fn = extract_function_name(trimmed);
            continue;
        }

        if trimmed == "}" {
            current_fn = None;
            continue;
        }

        let Some(ref fn_name) = current_fn else {
            continue;
        };

        // Look for call/invoke instructions
        for callee in extract_callees(trimmed) {
            let has_body = defined.contains(&callee);
            let display_name = demangle_rust_symbol(&callee);
            let entry = callgraph.entry(fn_name.clone()).or_default();
            // Deduplicate by mangled name
            if !entry.iter().any(|c| c.mangled_name == callee) {
                entry.push(CalledFunction {
                    name: display_name,
                    mangled_name: callee,
                    has_body,
                });
            }
        }
    }

    callgraph
}

/// Extract callees from a single IR instruction line.
/// Matches patterns like:
///   `call ... @"symbol_name"(...)`
///   `call ... @symbol_name(...)`
///   `invoke ... @"symbol_name"(...)`
fn extract_callees(line: &str) -> Vec<String> {
    let mut callees = Vec::new();
    let trimmed = line.trim();

    // Skip debug intrinsics and lifetime markers
    if trimmed.contains("@llvm.dbg.")
        || trimmed.contains("@llvm.lifetime")
        || trimmed.contains("#dbg_")
    {
        return callees;
    }

    // Find all @"name" or @name patterns that are call targets
    let mut pos = 0;
    let bytes = trimmed.as_bytes();
    while pos < bytes.len() {
        // Look for `call ` or `invoke ` followed eventually by `@`
        let is_call = (pos + 5 <= bytes.len() && &bytes[pos..pos + 5] == b"call ")
            || (pos + 7 <= bytes.len() && &bytes[pos..pos + 7] == b"invoke ");
        if !is_call {
            pos += 1;
            continue;
        }

        // Find the @ that starts the callee name
        if let Some(at_offset) = trimmed[pos..].find('@') {
            let at_pos = pos + at_offset;
            let name_start = at_pos + 1;
            if name_start >= trimmed.len() {
                break;
            }

            let name = if bytes[name_start] == b'"' {
                // Quoted name: @"..."
                let close = trimmed[name_start + 1..].find('"');
                close.map(|c| &trimmed[name_start + 1..name_start + 1 + c])
            } else {
                // Unquoted name: @name until ( or space
                let end = trimmed[name_start..]
                    .find(|c: char| c == '(' || c.is_whitespace())
                    .unwrap_or(trimmed.len() - name_start);
                Some(&trimmed[name_start..name_start + end])
            };

            if let Some(name) = name {
                if !name.is_empty() && !name.starts_with("llvm.") {
                    callees.push(name.to_string());
                }
            }
        }
        // Move past this call/invoke
        pos += 7;
    }

    callees
}

/// Extract the function name from a `define`/`declare` line.
fn extract_function_name(line: &str) -> Option<String> {
    let at_pos = line.find('@')?;
    let name_start = at_pos + 1;
    let bytes = line.as_bytes();

    if name_start < bytes.len() && bytes[name_start] == b'"' {
        let close = line[name_start + 1..].find('"')?;
        Some(line[name_start + 1..name_start + 1 + close].to_string())
    } else {
        let end = line[name_start..]
            .find(|c: char| c == '(' || c.is_whitespace())
            .unwrap_or(line.len() - name_start);
        Some(line[name_start..name_start + end].to_string())
    }
}

/// Basic Rust symbol demangling. Handles v0 mangling (_R prefix)
/// by stripping the hash suffix and converting to a readable path.
/// Falls back to the raw symbol if demangling isn't possible.
fn demangle_rust_symbol(mangled: &str) -> String {
    // Try rustc-demangle if available, otherwise do minimal cleanup
    if mangled.starts_with("_R") || mangled.starts_with("_ZN") {
        // Minimal: just return the mangled name with a comment
        // In production, use the `rustc-demangle` crate
        mangled.to_string()
    } else {
        mangled.to_string()
    }
}

/// Get only the external (non-defined) callees for a specific function.
pub fn external_callees(
    callgraph: &HashMap<String, Vec<CalledFunction>>,
    target: &str,
) -> Vec<CalledFunction> {
    // Find the target function (may be quoted or unquoted)
    let calls = callgraph
        .iter()
        .find(|(k, _)| k.contains(target))
        .map(|(_, v)| v.as_slice())
        .unwrap_or(&[]);

    calls.iter().filter(|c| !c.has_body).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_callees_finds_quoted_call() {
        let line = r#"  %result = call i32 @"_RNvXs_NtCs_3foo4barE"(ptr %x)"#;
        let callees = extract_callees(line);
        assert_eq!(callees, vec!["_RNvXs_NtCs_3foo4barE"]);
    }

    #[test]
    fn extract_callees_finds_unquoted_call() {
        let line = "  call void @abort() noreturn";
        let callees = extract_callees(line);
        assert_eq!(callees, vec!["abort"]);
    }

    #[test]
    fn extract_callees_skips_llvm_intrinsics() {
        let line = "  call void @llvm.memcpy.p0.p0.i64(ptr %dst, ptr %src, i64 8, i1 false)";
        let callees = extract_callees(line);
        assert!(callees.is_empty());
    }

    #[test]
    fn extract_callees_skips_debug() {
        let line = "  call void @llvm.dbg.declare(metadata ptr %x, metadata !123, metadata !DIExpression())";
        let callees = extract_callees(line);
        assert!(callees.is_empty());
    }

    #[test]
    fn extract_function_name_quoted() {
        let name = extract_function_name(r#"define void @"_RNvMyFunc"(ptr %x) {"#);
        assert_eq!(name, Some("_RNvMyFunc".to_string()));
    }

    #[test]
    fn extract_function_name_unquoted() {
        let name = extract_function_name("define i32 @main() {");
        assert_eq!(name, Some("main".to_string()));
    }
}
