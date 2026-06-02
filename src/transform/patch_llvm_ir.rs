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
//! ```text
//!     clang -S -emit-llvm  ...      # producing in.ll
//!     saw-spec-gen patch-llvm-ir --input in.ll --output out.ll \
//!         --strip-msvc-eh --poison-to-undef
//!     llvm-as out.ll -o module.bc
//! ```
//!
//! All edits are line-scoped: each function below walks `text` line
//! by line so multi-line regex pitfalls (greedy `[^\}]*` running
//! past a comdat boundary) are impossible.

use std::fs;
use std::path::Path;

use super::eh_globals;

/// Per-pass counters reported back to the caller.
#[derive(Debug, Default, Clone, Copy)]
pub struct PatchStats {
    pub eh_globals_stripped: usize,
    pub itanium_typeinfo_stripped: usize,
    pub poison_replaced: usize,
    pub nsw_nuw_stripped: usize,
    pub sat_intrinsics_expanded: usize,
    pub allocas_zeroed: usize,
}

/// Read `input`, apply all passes, and write the result to `output`.
/// `input` and `output` may point to the same file.
pub fn patch_llvm_ir_file(
    input: &Path,
    output: &Path,
    init_undef_allocas: bool,
) -> anyhow::Result<PatchStats> {
    let text = fs::read_to_string(input)?;
    let (patched, stats) = patch_llvm_ir(&text, init_undef_allocas);
    fs::write(output, patched)?;
    Ok(stats)
}

/// In-memory variant exposed for unit tests. Applies every
/// always-on pass unconditionally — each is a no-op when its
/// pattern is absent. The `init_undef_allocas` pass only runs when
/// the flag is set.
pub fn patch_llvm_ir(text: &str, init_undef_allocas: bool) -> (String, PatchStats) {
    let mut stats = PatchStats::default();
    let (out, n) = eh_globals::strip_msvc_eh_globals(text);
    stats.eh_globals_stripped = n;
    let (out, n) = eh_globals::strip_itanium_typeinfo_globals(&out);
    stats.itanium_typeinfo_stripped = n;
    let (out, n) = replace_poison_with_undef(&out);
    stats.poison_replaced = n;
    let (out, n) = strip_nsw_nuw_flags(&out);
    stats.nsw_nuw_stripped = n;
    let (out, n) = expand_uadd_sat_intrinsics(&out);
    stats.sat_intrinsics_expanded = n;
    let out = if init_undef_allocas {
        let (o, n) = zero_undef_allocas(&out);
        stats.allocas_zeroed = n;
        o
    } else {
        out
    };
    (out, stats)
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
/// ```text
///     %r = call iN @llvm.uadd.sat.iN(iN %a, iN %b)
///   →
///     %__sat_sum_r = add iN %a, %b
///     %__sat_ov_r  = icmp ult iN %__sat_sum_r, %a
///     %r           = select i1 %__sat_ov_r, iN -1, iN %__sat_sum_r
/// ```
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

// ── Pass 5: zero-initialise uninitialized allocas ──────────────

/// Pass 5 (opt-in): insert `store <ty> zeroinitializer` after every
/// static `alloca` so Crucible never loads `undef` from an
/// uninitialized stack slot. See the module-level docs for
/// soundness caveats.
fn zero_undef_allocas(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len() + text.len() / 8);
    for line in text.split_inclusive('\n') {
        buf.push_str(line);
        if let Some(store) = try_zero_alloca(line) {
            buf.push_str(&store);
            count += 1;
        }
    }
    (buf, count)
}

/// If `line` is a static `alloca`, return a `store zeroinitializer`
/// line to insert immediately after it. Returns `None` for
/// non-alloca lines or dynamic allocas (those with a count operand).
fn try_zero_alloca(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // Match: %<name> = alloca <type>[, align <N>]
    let rest = trimmed.strip_prefix('%')?;
    let eq_pos = rest.find(" = alloca ")?;
    let name = &rest[..eq_pos];
    let after_alloca = &rest[eq_pos + " = alloca ".len()..];

    // Split off the optional ", align N" suffix.
    let (alloca_type, align) = if let Some(ap) = after_alloca.rfind(", align ") {
        let align_digits = after_alloca[ap + ", align ".len()..].trim();
        let a: u32 = align_digits.parse().ok()?;
        (after_alloca[..ap].trim(), a)
    } else {
        (after_alloca.trim(), 1u32)
    };

    // Skip dynamic allocas: a comma at bracket-depth 0 inside the
    // type portion means there is an extra count operand.
    if has_unbracketed_comma(alloca_type) {
        return None;
    }

    let lead = &line[..line.len() - line.trim_start().len()];
    let nl = if line.ends_with('\n') { "\n" } else { "" };
    Some(format!(
        "{lead}store {alloca_type} zeroinitializer, ptr %{name}, align {align}{nl}"
    ))
}

/// Returns `true` if `s` contains a `,` that is not nested inside
/// `{}`, `[]`, `<>`, or `()`.
fn has_unbracketed_comma(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '{' | '[' | '<' | '(' => depth += 1,
            '}' | ']' | '>' | ')' => depth = (depth - 1).max(0),
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── init-undef-allocas pass ────────────────────────────

    #[test]
    fn zeros_simple_alloca() {
        let input = "  %5 = alloca %\"struct.std::in_place_t\", align 1\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("store %\"struct.std::in_place_t\" zeroinitializer, ptr %5, align 1"),
            "got: {out}"
        );
    }

    #[test]
    fn zeros_primitive_alloca() {
        let input = "  %retval = alloca i32, align 4\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("  store i32 zeroinitializer, ptr %retval, align 4"),
            "got: {out}"
        );
    }

    #[test]
    fn zeros_array_alloca() {
        let input = "  %arr = alloca [10 x i32], align 16\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("store [10 x i32] zeroinitializer, ptr %arr, align 16"),
            "got: {out}"
        );
    }

    #[test]
    fn skips_dynamic_alloca() {
        let input = "  %p = alloca i32, i32 %n, align 4\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn skips_non_alloca_lines() {
        let input = "  %x = add i32 %a, %b\n  store i32 0, ptr %y\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn alloca_no_align() {
        let input = "  %x = alloca i8\n";
        let (out, n) = zero_undef_allocas(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("store i8 zeroinitializer, ptr %x, align 1"),
            "got: {out}"
        );
    }

    #[test]
    fn pass_disabled_by_flag() {
        let input = "  %x = alloca i32, align 4\n";
        let (out, stats) = patch_llvm_ir(input, false);
        assert_eq!(stats.allocas_zeroed, 0);
        assert!(!out.contains("zeroinitializer"));
    }

    #[test]
    fn pass_enabled_by_flag() {
        let input = "  %x = alloca i32, align 4\n";
        let (out, stats) = patch_llvm_ir(input, true);
        assert_eq!(stats.allocas_zeroed, 1);
        assert!(out.contains("zeroinitializer"));
    }
}
