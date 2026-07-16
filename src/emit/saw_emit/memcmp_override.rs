//! Faithful fixed-length `memcmp` override emission.
//!
//! When [`crate::transform::memcmp_scan`] finds that every `memcmp`
//! call in the module passes the same constant length, this emits a
//! functional override (`rv == 0` iff the N bytes are equal) instead
//! of the adversarial havoc in [`super::bitcode_overrides`].

/// Emit a *faithful* fixed-length `memcmp` override for a call that
/// always passes the constant length `n`. The two `n`-byte buffers are
/// modeled as read-only symbolic arrays; the return is `0` exactly when
/// they are equal and `1` otherwise. This is sound for any caller that
/// only tests `memcmp(...) == 0` / `!= 0` (the overwhelmingly common
/// use); the specific magnitude/sign of the non-zero result is not
/// modeled. The concrete length term makes SAW's override matcher
/// require the call site's length argument to equal `n`.
pub(super) fn emit_faithful_memcmp(out: &mut String, safe: &str, ov_name: &str, n: u64) {
    out.push_str(&format!(
        "\n// override: memcmp  [faithful; fixed length {n}]\n"
    ));
    out.push_str(&format!("let {safe}_spec = do {{\n"));
    out.push_str(&format!(
        "    p0 <- llvm_alloc_readonly (llvm_array {n} (llvm_int 8));\n"
    ));
    out.push_str(&format!(
        "    a <- llvm_fresh_var \"a\" (llvm_array {n} (llvm_int 8));\n"
    ));
    out.push_str("    llvm_points_to p0 (llvm_term a);\n");
    out.push_str(&format!(
        "    p1 <- llvm_alloc_readonly (llvm_array {n} (llvm_int 8));\n"
    ));
    out.push_str(&format!(
        "    b <- llvm_fresh_var \"b\" (llvm_array {n} (llvm_int 8));\n"
    ));
    out.push_str("    llvm_points_to p1 (llvm_term b);\n");
    out.push_str(&format!(
        "    llvm_execute_func [p0, p1, llvm_term {{{{ {n} : [64] }}}}];\n"
    ));
    out.push_str("    llvm_return (llvm_term {{ if a == b then (0 : [32]) else (1 : [32]) }});\n");
    out.push_str("};\n");
    out.push_str(&format!(
        "{ov_name} <- llvm_unsafe_assume_spec m \"memcmp\" {safe}_spec;\n"
    ));
}

#[cfg(test)]
mod tests {
    use super::emit_faithful_memcmp;

    #[test]
    fn faithful_spec_models_equality_over_fixed_length() {
        let mut out = String::new();
        emit_faithful_memcmp(&mut out, "memcmp", "ov_memcmp", 16);
        assert!(out.contains("faithful; fixed length 16"));
        assert!(out.contains("llvm_array 16 (llvm_int 8)"));
        assert!(out.contains("if a == b then (0 : [32]) else (1 : [32])"));
        assert!(out.contains("llvm_execute_func [p0, p1, llvm_term {{ 16 : [64] }}]"));
        // A faithful spec must NOT havoc the return value.
        assert!(!out.contains("rv <- llvm_fresh_var"));
    }

    #[test]
    fn faithful_spec_binds_the_override_name() {
        let mut out = String::new();
        emit_faithful_memcmp(&mut out, "memcmp", "ov_memcmp", 8);
        assert!(out.contains("ov_memcmp <- llvm_unsafe_assume_spec m \"memcmp\" memcmp_spec;"));
    }
}
