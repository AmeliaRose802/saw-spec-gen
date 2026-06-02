//! Heuristic "Crucible-safe" check for LLVM IR function bodies.
//!
//! Background
//! ----------
//! `saw-spec-gen`'s compositional model used to bail out of any callee
//! whose declaration lived in a system header (libstdc++ / libc++ /
//! MSVC STL / CRT) and replace the call with an adversarial havoc
//! override. That blanket rule is too coarse for header-only STL
//! templates such as `std::max`, `std::min`, `std::clamp`,
//! `std::pair::first`, `std::tuple::get<I>` — clang instantiates their
//! bodies into the bitcode and the bodies are pure arithmetic +
//! comparisons that Crucible can lower natively.
//!
//! This module replaces the binary `is_system` gate with a two-step
//! gate:
//!
//!   1. Does the callee have a body in the IR? (Existing check.)
//!   2. Is the body built from opcodes Crucible can lower without an
//!      external override? (This module.)
//!
//! If yes, the call-graph walker recurses into the body; if no, the
//! adversarial-override fallback fires as before.
//!
//! See `crucible_safety_tests.rs` for the allow-list pinning tests.

use std::collections::{HashMap, HashSet};

/// Outcome of `is_crucible_safe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyReport {
    /// True iff every instruction in the (transitive) body is on the
    /// allow-list and every callee resolves to another safe body.
    pub safe: bool,
    /// First instruction line that violated the allow-list. Useful for
    /// `SAW_SPEC_GEN_DEBUG=1` diagnostics.
    pub first_unsafe_inst: Option<String>,
}

impl SafetyReport {
    fn safe() -> Self {
        Self {
            safe: true,
            first_unsafe_inst: None,
        }
    }
    fn unsafe_with(reason: impl Into<String>) -> Self {
        Self {
            safe: false,
            first_unsafe_inst: Some(reason.into()),
        }
    }
}

/// Memoising analyzer keyed by mangled symbol name. Reuse across many
/// callees in the same module — building the body index walks the IR
/// once.
pub struct SafetyAnalyzer<'a> {
    /// (start_line_index, end_line_index_exclusive) per defined symbol.
    bodies: HashMap<String, (usize, usize)>,
    /// Cached results so a hot symbol like `std::max<u32>` is only
    /// walked once even when many callers reference it.
    cache: HashMap<String, SafetyReport>,
    /// Line storage materialised lazily (split once, reused).
    lines: Vec<&'a str>,
}

impl<'a> SafetyAnalyzer<'a> {
    pub fn new(ir: &'a str) -> Self {
        let lines: Vec<&'a str> = ir.lines().collect();
        let bodies = index_bodies(&lines);
        Self {
            bodies,
            cache: HashMap::new(),
            lines,
        }
    }

    /// Returns whether `mangled` resolves to a body whose every
    /// instruction is Crucible-safe. Unknown symbols (no `define` in
    /// this module) return `unsafe` so the caller falls back to the
    /// adversarial-override path.
    pub fn is_safe(&mut self, mangled: &str) -> SafetyReport {
        let mut visited = HashSet::new();
        self.check(mangled, &mut visited)
    }

    fn check(&mut self, mangled: &str, visited: &mut HashSet<String>) -> SafetyReport {
        if let Some(cached) = self.cache.get(mangled) {
            return cached.clone();
        }
        // Recursion: a function that (transitively) calls itself can't
        // be unrolled by Crucible in finite time. Treat as unsafe.
        if !visited.insert(mangled.to_string()) {
            let r = SafetyReport::unsafe_with(format!("recursive call to @{mangled}"));
            self.cache.insert(mangled.to_string(), r.clone());
            return r;
        }
        let Some(&(start, end)) = self.bodies.get(mangled) else {
            // Declare-only (or absent) — caller will treat as external.
            let r = SafetyReport::unsafe_with(format!("no body for @{mangled} in module"));
            self.cache.insert(mangled.to_string(), r.clone());
            return r;
        };
        // Snapshot the body lines before recursing so we don't hold a
        // borrow on `self.lines` across `self.check`.
        let body: Vec<&'a str> = self.lines[start..end].to_vec();
        for raw in body {
            let inst = strip_inst_prefix(raw);
            if inst.is_empty() {
                continue;
            }
            match classify_instruction(inst) {
                InstClass::Safe => {}
                InstClass::Unsafe(reason) => {
                    let r = SafetyReport::unsafe_with(format!("{reason}: {}", inst.trim()));
                    self.cache.insert(mangled.to_string(), r.clone());
                    return r;
                }
                InstClass::Call(callees) => {
                    for callee in callees {
                        let sub = self.check(&callee, visited);
                        if !sub.safe {
                            let why = sub
                                .first_unsafe_inst
                                .unwrap_or_else(|| "unsafe transitively".to_string());
                            let r =
                                SafetyReport::unsafe_with(format!("transitive: @{callee} → {why}"));
                            self.cache.insert(mangled.to_string(), r.clone());
                            return r;
                        }
                    }
                }
            }
        }
        let r = SafetyReport::safe();
        self.cache.insert(mangled.to_string(), r.clone());
        r
    }
}

/// Convenience one-shot entry point — builds and discards an analyzer.
/// Prefer [`SafetyAnalyzer`] when checking many symbols against the
/// same IR module.
pub fn is_crucible_safe(mangled: &str, ir: &str, visited: &mut HashSet<String>) -> SafetyReport {
    let mut analyzer = SafetyAnalyzer::new(ir);
    analyzer.check(mangled, visited)
}

/// Walk the lines and record the `(start, end_exclusive)` index range
/// for each `define`'d function body (the lines between the opening
/// `{` and the matching closing `}`). Brace counting handles nested
/// types (`{ <i32, i32> }`) appearing on the same line.
fn index_bodies(lines: &[&str]) -> HashMap<String, (usize, usize)> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("define ") && line.trim_end().ends_with('{') {
            if let Some(name) = extract_function_symbol(trimmed) {
                let body_start = i + 1;
                let mut depth = 1;
                let mut j = body_start;
                while j < lines.len() && depth > 0 {
                    let t = lines[j].trim();
                    if t == "}" {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                out.insert(name, (body_start, j));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract the `@symbol` from a `define ... @"name"(...) {` header.
fn extract_function_symbol(define_line: &str) -> Option<String> {
    // Find the `@` that introduces the symbol name. Skip past anything
    // before it (attrs, return type).
    let at = define_line.find('@')?;
    let after = &define_line[at + 1..];
    let name = if let Some(rest) = after.strip_prefix('"') {
        // `@"mangled name"(`
        let end = rest.find('"')?;
        rest[..end].to_string()
    } else {
        // `@mangled(`
        let end = after
            .find(|c: char| c == '(' || c.is_whitespace())
            .unwrap_or(after.len());
        after[..end].to_string()
    };
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Strip leading whitespace + an optional SSA assignment prefix
/// (`%name = `). Drops comment-only lines and label lines.
fn strip_inst_prefix(line: &str) -> &str {
    let t = line.trim();
    if t.is_empty() {
        return "";
    }
    // Comments
    if t.starts_with(';') {
        return "";
    }
    // Basic-block labels look like `12:` or `entry:`.
    // We rely on the fact that real instructions never start with a
    // bare identifier followed by `:` at end of line.
    if !t.starts_with('%') && !t.starts_with('@') {
        if let Some(colon_pos) = t.find(':') {
            // Allow `:` inside type strings, but a label has the colon
            // either at end-of-line or immediately before whitespace/
            // comment. Cheap heuristic: label has no `=` and no `(`.
            let head = &t[..colon_pos];
            if !head.contains('=')
                && !head.contains('(')
                && !head.contains(' ')
                && head
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                return "";
            }
        }
    }
    // `%result = <inst>`
    if let Some(eq) = t.find('=') {
        // Must look like `%xxx = ...` (no spaces in the lhs).
        let lhs = t[..eq].trim();
        if (lhs.starts_with('%') || lhs.starts_with('@')) && !lhs.contains(' ') {
            return t[eq + 1..].trim_start();
        }
    }
    t
}

enum InstClass {
    Safe,
    Unsafe(&'static str),
    /// Mangled callee names to recurse into (always one entry for
    /// `call`/`invoke`, but kept as a `Vec` to make extension trivial).
    Call(Vec<String>),
}

/// Allow-list opcodes (no operand parsing required).
const SAFE_OPCODES: &[&str] = &[
    // arithmetic / bit / compare
    "add",
    "sub",
    "mul",
    "udiv",
    "sdiv",
    "urem",
    "srem",
    "fadd",
    "fsub",
    "fmul",
    "fdiv",
    "frem",
    "shl",
    "lshr",
    "ashr",
    "and",
    "or",
    "xor",
    "icmp",
    "fcmp",
    "select",
    "phi",
    "freeze",
    // conversions
    "trunc",
    "zext",
    "sext",
    "fptrunc",
    "fpext",
    "sitofp",
    "uitofp",
    "fptosi",
    "fptoui",
    "bitcast",
    "ptrtoint",
    "inttoptr",
    "addrspacecast",
    // memory
    "alloca",
    "load",
    "store",
    "getelementptr",
    "extractvalue",
    "insertvalue",
    "extractelement",
    "insertelement",
    "shufflevector",
    // control flow
    "br",
    "switch",
    "indirectbr",
    "ret",
    "unreachable",
];

/// Disallowed opcodes — even if a body looks otherwise pure, presence
/// of any of these forces fallback to an adversarial override.
const UNSAFE_OPCODES: &[&str] = &[
    // exception handling (handled separately by the exception-lower pass)
    "invoke",
    "landingpad",
    "resume",
    "catchpad",
    "catchswitch",
    "cleanuppad",
    "cleanupret",
    "catchret",
    // concurrency
    "atomicrmw",
    "cmpxchg",
    "fence",
    // varargs
    "va_arg",
];

/// Pure intrinsics whose call sites are safe without recursion.
const SAFE_INTRINSICS: &[&str] = &[
    "llvm.fabs.",
    "llvm.sqrt.",
    "llvm.abs.",
    "llvm.umin.",
    "llvm.umax.",
    "llvm.smin.",
    "llvm.smax.",
    "llvm.bswap.",
    "llvm.ctlz.",
    "llvm.cttz.",
    "llvm.ctpop.",
    "llvm.fshl.",
    "llvm.fshr.",
    "llvm.assume",
    "llvm.expect.",
    "llvm.lifetime.",
    "llvm.dbg.",
    "llvm.memcpy.",
    "llvm.memmove.",
    "llvm.memset.",
    "llvm.invariant.",
    "llvm.launder.invariant",
    "llvm.strip.invariant",
];

fn classify_instruction(inst: &str) -> InstClass {
    // Pick the opcode token. Skip leading attribute keywords
    // (`tail`, `musttail`, `notail`) that precede `call`.
    let mut tokens = inst.split_whitespace();
    let mut op = tokens.next().unwrap_or("");
    while matches!(op, "tail" | "musttail" | "notail") {
        op = tokens.next().unwrap_or("");
    }
    if op.is_empty() {
        return InstClass::Safe;
    }
    if op == "call" || op == "invoke" {
        // `invoke` is disallowed via the unsafe list, but we may still
        // want to extract the callee for diagnostics.
        if op == "invoke" {
            return InstClass::Unsafe("invoke (EH)");
        }
        return classify_call(inst);
    }
    if SAFE_OPCODES.contains(&op) {
        return InstClass::Safe;
    }
    if UNSAFE_OPCODES.contains(&op) {
        return InstClass::Unsafe(disallowed_reason(op));
    }
    // Unknown opcode — be conservative.
    InstClass::Unsafe("unknown opcode")
}

fn disallowed_reason(op: &str) -> &'static str {
    match op {
        "invoke" => "invoke (EH)",
        "landingpad" | "resume" | "catchpad" | "catchswitch" | "cleanuppad" | "cleanupret"
        | "catchret" => "exception handling",
        "atomicrmw" | "cmpxchg" | "fence" => "atomic/fence",
        "va_arg" => "varargs",
        _ => "disallowed opcode",
    }
}

/// Extract a callee `@symbol` from a `call ...` instruction line.
/// Returns `Unsafe` for inline asm and for any intrinsic not on the
/// allow-list, `Safe` for allow-listed intrinsics, and `Call([sym])`
/// for everything else.
fn classify_call(inst: &str) -> InstClass {
    if inst.contains(" asm ") || inst.contains("asm sideeffect") || inst.contains(" asm\t") {
        return InstClass::Unsafe("inline asm");
    }
    // Find `@"name"(` or `@name(`. Choose the **rightmost** `@` token
    // that is followed eventually by `(`, because earlier `@` tokens
    // may appear inside metadata or attribute groups (rare in real
    // bodies, but cheap to guard against).
    let Some(at) = inst.rfind('@') else {
        // Indirect call through a `%reg` — we can't know what it is.
        // Treat as unsafe; this catches function pointers.
        return InstClass::Unsafe("indirect call");
    };
    let after = &inst[at + 1..];
    let name = if let Some(rest) = after.strip_prefix('"') {
        let Some(end) = rest.find('"') else {
            return InstClass::Unsafe("malformed callee");
        };
        rest[..end].to_string()
    } else {
        let end = after
            .find(|c: char| c == '(' || c.is_whitespace())
            .unwrap_or(after.len());
        after[..end].to_string()
    };
    if name.starts_with("llvm.") {
        if SAFE_INTRINSICS.iter().any(|&s| name.starts_with(s)) {
            return InstClass::Safe;
        }
        // llvm.va_start, llvm.va_end, llvm.eh.*, llvm.stackrestore, etc.
        return InstClass::Unsafe("disallowed intrinsic");
    }
    InstClass::Call(vec![name])
}

#[cfg(test)]
#[path = "crucible_safety_tests.rs"]
mod tests;
