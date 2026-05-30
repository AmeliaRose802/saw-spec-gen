//! Shared constants used by the various spec-emission modules.

/// SAW type sentinel emitted for `void`-returning functions. We use a
/// comment prefix so the value is harmless if it ever leaks into a
/// generated SAW script verbatim.
pub const VOID_SAW_TYPE: &str = "// void";

/// Returns `true` when a SAW type string represents `void` — i.e. the
/// function has no return value to bind. Centralizing this check keeps
/// the spec-emitters honest about what counts as a void return.
pub fn is_void_saw_type(saw_type: &str) -> bool {
    saw_type == VOID_SAW_TYPE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn void_constant_is_stable() {
        assert_eq!(VOID_SAW_TYPE, "// void");
        assert!(is_void_saw_type("// void"));
        assert!(!is_void_saw_type("llvm_int 32"));
        assert!(!is_void_saw_type("// VOID")); // case-sensitive
    }
}
