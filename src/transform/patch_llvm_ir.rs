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

/// Per-pass counters reported back to the caller.
#[derive(Debug, Default, Clone, Copy)]
pub struct PatchStats {
    pub eh_globals_stripped: usize,
    pub poison_replaced: usize,
    pub nsw_nuw_stripped: usize,
    pub sat_intrinsics_expanded: usize,
}

/// Read `input`, apply all passes, and write the result to `output`.
/// `input` and `output` may point to the same file.
pub fn patch_llvm_ir_file(input: &Path, output: &Path) -> anyhow::Result<PatchStats> {
    let text = fs::read_to_string(input)?;
    let (patched, stats) = patch_llvm_ir(&text);
    fs::write(output, patched)?;
    Ok(stats)
}

/// In-memory variant exposed for unit tests. Applies every pass
/// unconditionally — each is a no-op when its pattern is absent.
pub fn patch_llvm_ir(text: &str) -> (String, PatchStats) {
    let mut stats = PatchStats::default();
    let (out, n) = strip_msvc_eh_globals(text);
    stats.eh_globals_stripped = n;
    let (out, n) = replace_poison_with_undef(&out);
    stats.poison_replaced = n;
    let (out, n) = strip_nsw_nuw_flags(&out);
    stats.nsw_nuw_stripped = n;
    let (out, n) = expand_uadd_sat_intrinsics(&out);
    stats.sat_intrinsics_expanded = n;
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
    let inner = name_with_at.trim_start_matches('@').trim_matches('"');
    if inner.starts_with("_CT??_R0") || inner.starts_with("_TI") || inner.starts_with("_CTA") {
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

/// Pass 3: strip `nsw` and `nuw` instruction flags from arithmetic
/// and cast instructions. `opt -O1` adds these flags to communicate
/// "the compiler proved no overflow here", but SAW / Crucible
/// treats them strictly — an `add nsw` whose operands *could*
/// overflow (in the solver's unconstrained view of that instruction
/// in isolation) produces poison. This causes false DISPROVED
/// results for code that is logically correct.
///
/// We match ` nsw` and ` nuw` as whole words on instruction lines
/// (lines starting with optional whitespace then `%` or a keyword
/// like `store`, `ret`, etc.) and remove them. Comment lines and
/// metadata are left untouched.
fn strip_nsw_nuw_flags(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        // Only process instruction lines (start with % or a
        // known opcode).  Skip comments, metadata, globals, and
        // attribute groups.
        let is_instruction = trimmed.starts_with('%')
            || trimmed.starts_with("ret ")
            || trimmed.starts_with("store ")
            || trimmed.starts_with("br ");
        if is_instruction || trimmed.contains(" = ") {
            let mut patched = line.to_string();
            // Handle both orderings: "nsw nuw", "nuw nsw", lone
            // "nsw", lone "nuw".  Loop until no more replacements.
            loop {
                let before = patched.len();
                patched = patched.replace(" nsw nuw ", " ");
                patched = patched.replace(" nuw nsw ", " ");
                patched = patched.replace(" nsw ", " ");
                patched = patched.replace(" nuw ", " ");
                if patched.len() == before {
                    break;
                }
                count += 1;
            }
            buf.push_str(&patched);
        } else {
            buf.push_str(line);
        }
    }
    (buf, count)
}

/// Pass 4: expand `@llvm.uadd.sat.iN` intrinsic calls into basic
/// instructions that SAW / Crucible can handle.
///
/// `clang -O1` on Linux (Itanium ABI) recognizes the saturating-add
/// pattern and lowers it to a single `@llvm.uadd.sat.iN` call.
/// SAW doesn't support this intrinsic, so we expand it back:
///
///     %r = call iN @llvm.uadd.sat.iN(iN %a, iN %b)
///   →
///     %__sat_sum_r = add iN %a, %b
///     %__sat_ov_r  = icmp ult iN %__sat_sum_r, %a
///     %r           = select i1 %__sat_ov_r, iN -1, iN %__sat_sum_r
fn expand_uadd_sat_intrinsics(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if let Some(expansion) = try_expand_uadd_sat(line) {
            buf.push_str(&expansion);
            count += 1;
        } else {
            buf.push_str(line);
        }
    }
    (buf, count)
}

/// Try to expand a single `@llvm.uadd.sat.iN` call line. Returns
/// `None` if the line is not a matching call.
fn try_expand_uadd_sat(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.contains("@llvm.uadd.sat.i") {
        return None;
    }
    let eq_pos = trimmed.find(" = ")?;
    let dest = trimmed[..eq_pos].trim();
    if !dest.starts_with('%') {
        return None;
    }
    // Extract type from the intrinsic name: @llvm.uadd.sat.i<N>
    let marker = "@llvm.uadd.sat.";
    let type_start = trimmed.find(marker)? + marker.len();
    let paren = trimmed[type_start..].find('(')?;
    let ty = &trimmed[type_start..type_start + paren]; // e.g. "i32"
    if !ty.starts_with('i') || ty[1..].parse::<u32>().is_err() {
        return None;
    }
    // Parse operands from parentheses
    let args_start = type_start + paren + 1;
    let args_end = trimmed[args_start..].find(')')?;
    let args_str = &trimmed[args_start..args_start + args_end];
    let parts: Vec<&str> = args_str.splitn(2, ',').collect();
    if parts.len() != 2 {
        return None;
    }
    let op_a = extract_llvm_operand(parts[0].trim(), ty)?;
    let op_b = extract_llvm_operand(parts[1].trim(), ty)?;
    let suffix = dest.trim_start_matches('%');
    let lead = &line[..line.len() - line.trim_start().len()];
    let nl = if line.ends_with('\n') { "\n" } else { "" };
    Some(format!(
        "{lead}%__sat_sum_{suffix} = add {ty} {op_a}, {op_b}{nl}\
         {lead}%__sat_ov_{suffix} = icmp ult {ty} %__sat_sum_{suffix}, {op_a}{nl}\
         {lead}{dest} = select i1 %__sat_ov_{suffix}, {ty} -1, {ty} %__sat_sum_{suffix}{nl}"
    ))
}

/// Extract the value operand from e.g. `"i32 noundef %0"`, skipping
/// the type and optional attributes. Returns `None` if the type
/// prefix doesn't match `expected_ty`.
fn extract_llvm_operand<'a>(s: &'a str, expected_ty: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(expected_ty)?.trim_start();
    // Skip optional attributes like `noundef`
    let rest = if let Some(r) = rest.strip_prefix("noundef") {
        r.trim_start()
    } else {
        rest
    };
    if rest.is_empty() {
        return None;
    }
    Some(rest)
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

    #[test]
    fn expands_uadd_sat_i32() {
        let input = "  %3 = tail call i32 @llvm.uadd.sat.i32(i32 noundef %0, i32 noundef %1)\n";
        let (out, n) = expand_uadd_sat_intrinsics(input);
        assert_eq!(n, 1);
        assert!(out.contains("%__sat_sum_3 = add i32 %0, %1"), "got: {out}");
        assert!(
            out.contains("%__sat_ov_3 = icmp ult i32 %__sat_sum_3, %0"),
            "got: {out}"
        );
        assert!(
            out.contains("%3 = select i1 %__sat_ov_3, i32 -1, i32 %__sat_sum_3"),
            "got: {out}"
        );
    }

    #[test]
    fn expands_uadd_sat_no_noundef() {
        let input = "  %r = call i64 @llvm.uadd.sat.i64(i64 %a, i64 %b)\n";
        let (out, n) = expand_uadd_sat_intrinsics(input);
        assert_eq!(n, 1);
        assert!(out.contains("add i64 %a, %b"), "got: {out}");
        assert!(
            out.contains("select i1 %__sat_ov_r, i64 -1, i64 %__sat_sum_r"),
            "got: {out}"
        );
    }

    #[test]
    fn leaves_non_sat_intrinsics_alone() {
        let input = "  %1 = call i32 @llvm.abs.i32(i32 %0, i1 false)\n";
        let (out, n) = expand_uadd_sat_intrinsics(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }
}
