//! Extract the constant length argument from `memcmp` call sites.
//!
//! When every `memcmp` call in a module passes the same compile-time
//! literal length, [`crate::emit::saw_emit::bitcode_overrides`] can
//! emit a *faithful* fixed-length override (`rv == 0` iff the N bytes
//! are equal) instead of an adversarial havoc that lets the solver
//! pick `rv == 0` for unequal buffers (a false DISPROVED) or nonzero
//! for equal buffers (a false VERIFIED).

/// Scan the module IR for `memcmp` call sites and, when every call
/// passes the SAME compile-time-constant length argument (and at least
/// one call exists), return it. If any call site passes a non-constant
/// (register/`%`) length, or two call sites disagree on the literal,
/// return `None` so the emitter falls back to the adversarial havoc.
///
/// The length is the third (last) argument of `call ... @memcmp(ptr
/// ..., ptr ..., i64 <LEN>)`. A faithful fixed-length spec built from
/// the returned `n` is only sound when the call site truly uses that
/// length; requiring a *unique literal across the whole module* is the
/// conservative condition under which that holds.
pub(crate) fn memcmp_const_len_from_ir(ir: &str) -> Option<u64> {
    let mut found: Option<u64> = None;
    for line in ir.lines() {
        let Some(idx) = line.find("@memcmp(") else {
            continue;
        };
        // Only inspect actual call/invoke sites — the `declare i32
        // @memcmp(ptr, ptr, i64)` line also contains `@memcmp(` but has
        // no length argument and would spuriously poison the result.
        if !line[..idx].contains("call ") && !line[..idx].contains("invoke ") {
            continue;
        }
        let args = &line[idx + "@memcmp(".len()..];
        // Fall back to havoc on the first non-constant length.
        let len = memcmp_len_arg(args)?;
        match found {
            Some(n) if n != len => return None,
            _ => found = Some(len),
        }
    }
    found
}

/// Parse the length (third) argument out of a `@memcmp(...)` argument
/// list. `memcmp`'s arguments contain no nested parentheses, so the
/// first `)` closes the call. Returns `None` when the last argument is
/// not a plain integer literal (e.g. `i64 noundef %25`).
fn memcmp_len_arg(args: &str) -> Option<u64> {
    let end = args.find(')')?;
    let last = args[..end].rsplit(',').next()?.trim();
    // last is like "i64 noundef 16" or "i64 16" or "i64 noundef %25".
    last.split_whitespace().last()?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_unique_literal() {
        let ir = r#"
define i32 @target(ptr %a, ptr %b) {
  %1 = call i32 @memcmp(ptr noundef %a, ptr noundef %b, i64 noundef 16)
  ret i32 %1
}
declare i32 @memcmp(ptr, ptr, i64)
"#;
        assert_eq!(memcmp_const_len_from_ir(ir), Some(16));
    }

    #[test]
    fn none_for_symbolic_length() {
        // The length is a register (`%n`), as MSVC STL emits for
        // `std::equal` over raw pointer ranges (pointer-difference).
        // Must fall back to havoc, not a bogus constant.
        let ir = r#"
define i32 @target(ptr %a, ptr %b, i64 %n) {
  %1 = call i32 @memcmp(ptr noundef %a, ptr noundef %b, i64 noundef %n)
  ret i32 %1
}
declare i32 @memcmp(ptr, ptr, i64)
"#;
        assert_eq!(memcmp_const_len_from_ir(ir), None);
    }

    #[test]
    fn none_when_sites_disagree() {
        let ir = r#"
define i32 @target(ptr %a, ptr %b) {
  %1 = call i32 @memcmp(ptr noundef %a, ptr noundef %b, i64 noundef 16)
  %2 = call i32 @memcmp(ptr noundef %a, ptr noundef %b, i64 noundef 32)
  ret i32 %1
}
declare i32 @memcmp(ptr, ptr, i64)
"#;
        assert_eq!(memcmp_const_len_from_ir(ir), None);
    }

    #[test]
    fn none_when_no_call_sites() {
        // Only a declaration, no call — nothing to model faithfully.
        let ir = "declare i32 @memcmp(ptr, ptr, i64)\n";
        assert_eq!(memcmp_const_len_from_ir(ir), None);
    }
}
