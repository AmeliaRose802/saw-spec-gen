//! Textual rewrites applied to a `.ll` file before SAW loads the
//! bitcode. Each pass fixes a known interaction between modern
//! `clang`/`rustc` output and the SAW 1.5 / Crucible LLVM frontend.
//!
//! Two passes are currently implemented, both opt-in via flags on
//! the `patch-llvm-ir` CLI subcommand:
//!
//! * `strip_msvc_eh` -- replace MSVC C++ exception-handling
//!   metadata globals (the `_TI`, `_CTA`, `_CT??_R0*` chain emitted
//!   in `section ".xdata"`) with `external constant` declarations.
//!   Those globals' initializers use `ptrtoint` differences against
//!   `@__ImageBase` for image-relative pointer encoding, which the
//!   Crucible-LLVM constant evaluator rejects with
//!   `Illegal operation applied to pointer argument` at module load
//!   time. The metadata is only ever read by the OS unwinder, never
//!   by user code, so dropping the initializer is sound for the
//!   `_CxxThrowException` call site that references the head of the
//!   chain.
//!
//! * `poison_to_undef` -- replace every occurrence of the LLVM 15+
//!   token `poison` with `undef`. `rustc` (and recent `clang`)
//!   emit `insertvalue { i32 0, i32 poison }, ...` style aggregates
//!   for tuple/Poll returns; Crucible's `llvmExtensionEval` panics
//!   with "Attempting to evaluate poison value" when it materialises
//!   the base aggregate before the `insertvalue`. `undef` is
//!   semantically weaker (it commutes with selects/inserts cleanly)
//!   and Crucible handles it without trouble. The substitution is
//!   safe whenever the poison field is overwritten before any use,
//!   which is exactly the rustc/clang idiom we hit.
//!
//! The passes are deliberately string-level: they operate on the
//! disassembled `.ll` so callers don't need an LLVM library
//! linked. The expected wrapper flow is
//!
//!     clang -S -emit-llvm  ...      # producing in.ll
//!     saw-spec-gen patch-llvm-ir --input in.ll --output out.ll \
//!         --strip-msvc-eh --poison-to-undef
//!     llvm-as out.ll -o module.bc
//!
//! All edits are line-scoped: each function below walks `text` line
//! by line so multi-line regex pitfalls (greedy `[^\}]*` running
//! past a comdat boundary) are impossible.

use std::fs;
use std::path::Path;

/// Configuration for [`patch_llvm_ir`]. Each flag turns on one
/// independent pass; passes commute and may be enabled together.
#[derive(Debug, Default, Clone, Copy)]
pub struct PatchOptions {
    /// Replace MSVC EH metadata globals with `external constant`.
    pub strip_msvc_eh: bool,
    /// Replace `poison` literals with `undef`.
    pub poison_to_undef: bool,
}

/// Per-pass counters reported back to the caller. Used by the CLI
/// to log e.g. "Stripped 3 MSVC EH globals; replaced 7 poison
/// literals".
#[derive(Debug, Default, Clone, Copy)]
pub struct PatchStats {
    pub eh_globals_stripped: usize,
    pub poison_replaced: usize,
}

/// Read `input`, apply the configured passes, and write the result
/// to `output`. `input` and `output` may point to the same file.
pub fn patch_llvm_ir_file(
    input: &Path,
    output: &Path,
    opts: PatchOptions,
) -> anyhow::Result<PatchStats> {
    let text = fs::read_to_string(input)?;
    let (patched, stats) = patch_llvm_ir(&text, opts);
    fs::write(output, patched)?;
    Ok(stats)
}

/// In-memory variant exposed for unit tests. Returns `(patched, stats)`.
pub fn patch_llvm_ir(text: &str, opts: PatchOptions) -> (String, PatchStats) {
    let mut stats = PatchStats::default();
    let mut out = text.to_string();
    if opts.strip_msvc_eh {
        let (s, n) = strip_msvc_eh_globals(&out);
        out = s;
        stats.eh_globals_stripped = n;
    }
    if opts.poison_to_undef {
        let (s, n) = replace_poison_with_undef(&out);
        out = s;
        stats.poison_replaced = n;
    }
    (out, stats)
}

/// Pass 1: replace MSVC EH-metadata global *definitions* with
/// `external constant` *declarations*.
///
/// We identify a global as MSVC EH metadata when both:
///
/// 1. its name matches the MSVC scheme for throw-info / catchable-type
///    chains (`_TI*`, `_CTA*`, `_CT??_R0*`), and
/// 2. the line places it in `section ".xdata"` (the MSVC exception
///    data section).
///
/// Requiring (2) means a stray user global called `_TIstats` (vanishingly
/// unlikely but legal) is not affected.
///
/// The output declaration preserves the original LLVM type so any
/// downstream use that takes the global's address (e.g. the
/// `_CxxThrowException` call) still type-checks.
fn strip_msvc_eh_globals(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if is_msvc_eh_global(line) {
            if let Some(decl) = rewrite_eh_global(line) {
                buf.push_str(&decl);
                count += 1;
                continue;
            }
        }
        buf.push_str(line);
    }
    (buf, count)
}

/// True iff `line` looks like a top-level MSVC EH-metadata global
/// definition. The check is intentionally conservative: it requires
/// both an EH-metadata name *and* the `section ".xdata"` clause.
fn is_msvc_eh_global(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    if !trimmed.contains("section \".xdata\"") {
        return false;
    }
    eh_global_name(trimmed).is_some()
}

/// Extract the global's name (including the leading `@`) if it
/// matches an MSVC EH-metadata scheme. Returns `None` otherwise.
fn eh_global_name(line: &str) -> Option<&str> {
    // Two forms: `@"_CT??_R0H@84"` (quoted, contains `??`) and
    // `@_TI1H` / `@_CTA1H` (unquoted, ASCII identifier).
    let rest = line.strip_prefix('@')?;
    let (name_with_at, _) = if let Some(rest) = rest.strip_prefix('"') {
        // Quoted name: read until the closing quote.
        let end = rest.find('"')?;
        let full = &line[..1 + 1 + end + 1]; // '@"' + body + '"'
        (full, &line[full.len()..])
    } else {
        // Unquoted: identifier characters only.
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let full = &line[..1 + end];
        (full, &line[full.len()..])
    };
    let inner = name_with_at
        .trim_start_matches('@')
        .trim_matches('"');
    if inner.starts_with("_CT??_R0")
        || inner.starts_with("_TI")
        || inner.starts_with("_CTA")
    {
        Some(name_with_at)
    } else {
        None
    }
}

/// Rewrite a single EH-metadata global definition line as an
/// `external constant <type>` declaration, preserving the LLVM
/// type. Returns `None` if the line doesn't parse as expected
/// (in which case the caller should leave it untouched).
fn rewrite_eh_global(line: &str) -> Option<String> {
    let name = eh_global_name(line.trim_start())?;
    // The type sits between the linkage/attribute words and the
    // `{` that starts the initializer. We do a deterministic walk:
    //   * find " constant " (with the leading space so we don't
    //     match a substring like `unnamed_addrconstant`),
    //   * the type is from there up to the `{` that opens the
    //     initializer.
    // If the line uses `global` (non-constant) instead of
    // `constant`, we still treat it as constant in the output --
    // the EH metadata is read-only.
    let const_idx = line
        .find(" constant ")
        .map(|i| i + " constant ".len())
        .or_else(|| line.find(" global ").map(|i| i + " global ".len()))?;
    let brace_idx = line[const_idx..].find('{')?;
    let ty = line[const_idx..const_idx + brace_idx].trim();
    if ty.is_empty() {
        return None;
    }
    // Preserve any leading whitespace from the original line so
    // diffs stay clean.
    let lead_ws_end = line.len() - line.trim_start().len();
    let leading = &line[..lead_ws_end];
    Some(format!("{leading}{name} = external constant {ty}\n"))
}

/// Pass 2: replace the `poison` token with `undef`. We only match
/// `poison` as a *whole word* (preceded by start-of-string, whitespace,
/// `,`, `(`, or `<`, and followed by whitespace, `,`, `)`, `>`, `]`, `}`,
/// or end-of-string) so identifiers or string literals containing
/// the substring `poison` are not touched.
fn replace_poison_with_undef(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"poison")
            && is_word_boundary_before(bytes, i)
            && is_word_boundary_after(bytes, i + b"poison".len())
        {
            out.push_str("undef");
            i += b"poison".len();
            count += 1;
        } else {
            // Push the next UTF-8 character to keep us aligned on
            // multi-byte boundaries (LLVM `.ll` files are ASCII in
            // practice but be safe).
            let ch_end = (1..=4)
                .map(|n| i + n)
                .find(|&n| n > bytes.len() || text.is_char_boundary(n))
                .unwrap_or(i + 1);
            out.push_str(&text[i..ch_end]);
            i = ch_end;
        }
    }
    (out, count)
}

fn is_word_boundary_before(bytes: &[u8], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let c = bytes[i - 1];
    !matches!(c, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'"')
}

fn is_word_boundary_after(bytes: &[u8], i: usize) -> bool {
    if i >= bytes.len() {
        return true;
    }
    let c = bytes[i];
    !matches!(c, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_throwinfo_global() {
        let input = "\
@_TI1H = linkonce_odr unnamed_addr constant %eh.ThrowInfo { i32 0, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @_CTA1H to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, section \".xdata\", comdat
";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@_TI1H = external constant %eh.ThrowInfo"),
            "got: {out}"
        );
        assert!(!out.contains("ptrtoint"));
    }

    #[test]
    fn strips_quoted_catchabletype_global() {
        let input = "@\"_CT??_R0H@84\" = linkonce_odr unnamed_addr constant %eh.CatchableType { i32 1, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @\"??_R0H@8\" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0, i32 -1, i32 0, i32 4, i32 0 }, section \".xdata\", comdat\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@\"_CT??_R0H@84\" = external constant %eh.CatchableType"),
            "got: {out}"
        );
    }

    #[test]
    fn leaves_non_xdata_global_alone() {
        // The RTTI TypeDescriptor (`@"??_R0H@8"`) lives in the
        // default section -- we must NOT rewrite it.
        let input = "@\"??_R0H@8\" = linkonce_odr global %rtti.TypeDescriptor2 { ptr @\"??_7type_info@@6B@\", ptr null, [3 x i8] c\".H\\00\" }, comdat\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn leaves_user_globals_alone() {
        let input = "@my_global = linkonce_odr constant i32 42\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn replaces_poison_word() {
        let input = "%1 = insertvalue { i32, i32 } { i32 0, i32 poison }, i32 %x, 1\n";
        let (out, n) = replace_poison_with_undef(input);
        assert_eq!(n, 1);
        assert!(out.contains("i32 undef"));
        assert!(!out.contains(" poison"));
    }

    #[test]
    fn leaves_identifiers_containing_poison_alone() {
        // A function or var named e.g. `poison_pill` must not be
        // touched.
        let input = "%poison_pill = alloca i32\n@poisonous = global i32 0\n";
        let (out, n) = replace_poison_with_undef(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }
}
