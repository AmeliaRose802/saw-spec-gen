//! Curated allowlist of MSVC `_Mutex_base` internal helpers whose
//! defined bodies (typically emitted as `linkonce_odr`) cause SAW
//! symbolic-execution failures.
//!
//! ## Why defined bodies need overrides here
//!
//! `_Mtx_lock` / `_Mtx_unlock` are **declare-only** UCRT externs
//! and already receive assumed overrides via
//! [`crate::emit::saw_emit::status_primitives::success_sentinel`].
//!
//! `std::_Mutex_base` accessor helpers — notably
//! `?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ` — are a
//! separate problem: they are **defined in-module** by the MSVC STL
//! headers as `linkonce_odr`, so they survive into the bitcode that
//! SAW loads. Their bodies perform typed reads of `_Mutex_base`'s
//! ownership and level fields. When the `KeyStore` object is modelled
//! as a flat byte buffer for sequential-transition proofs (`this=N`
//! bytes), those fields are never individually constrained, and the
//! typed `load` instruction hits unconstrained memory and aborts:
//!
//! ```text
//! internal: error: in ?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ
//! Error during memory load
//! ```
//!
//! Overriding these helpers with an adversarial havoc spec — the same
//! treatment as any other broken callee — is sound for sequential
//! proofs because:
//!
//!   * SAW verifies the *sequential* (single-threaded) transition;
//!     concurrency and ownership-level semantics are out of scope.
//!   * The helpers exist only as debug/consistency assertions in the
//!     MSVC runtime; they return `bool` but callers discard the
//!     return value and continue regardless.
//!   * Overriding them does not remove any real dataflow that
//!     influences the post-state modelled by the Cryptol spec.
//!
//! ## Pattern strategy
//!
//! MSVC mangles member functions of `std::_Mutex_base` with the
//! class qualifier `_Mutex_base@std` embedded in the symbol name.
//! A single substring match is sufficient to catch all current and
//! future helpers of this class without enumerating mangled variants
//! per calling-convention or architecture.
//!
//! This is intentionally a **conservative over-approximation**: it is
//! safer to havoc a helper we didn't strictly need to override than to
//! leave one unoverrideable and let the proof abort. Any false positive
//! (a `_Mutex_base@std` method whose body is actually tractable) is
//! sound — it just gets an adversarial spec instead of simulation.

/// Returns `true` if `mangled` is a known MSVC `_Mutex_base` internal
/// helper that should receive a forced override even when its body is
/// **defined** in the bitcode.
///
/// Currently matches any method of `std::_Mutex_base` (MSVC mangling
/// embeds `_Mutex_base@std` in the symbol name).
pub fn is_msvc_mutex_helper(mangled: &str) -> bool {
    // `_Mutex_base@std` appears in every MSVC-mangled member of
    // std::_Mutex_base, e.g.:
    //   ?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ
    //   ?_Verify_ownership_level@_Mutex_base@std@@IEAA_NXZ
    //   ?lock@_Mutex_base@std@@QEAAXXZ
    mangled.contains("_Mutex_base@std")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_ownership_levels_matches() {
        assert!(
            is_msvc_mutex_helper("?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"),
            "primary failure symbol must match"
        );
    }

    #[test]
    fn other_mutex_base_members_match() {
        assert!(is_msvc_mutex_helper(
            "?_Verify_ownership_level@_Mutex_base@std@@IEAA_NXZ"
        ));
        assert!(is_msvc_mutex_helper("?lock@_Mutex_base@std@@QEAAXXZ"));
    }

    #[test]
    fn unrelated_mutex_symbols_do_not_match() {
        // _Mtx_lock is handled by status_primitives, not this module.
        assert!(!is_msvc_mutex_helper("_Mtx_lock"));
        assert!(!is_msvc_mutex_helper("_Mtx_unlock"));
        // Some random symbol containing "mutex" but not the class.
        assert!(!is_msvc_mutex_helper("acquire_mutex_handle"));
        assert!(!is_msvc_mutex_helper("std_mutex_lock"));
    }

    #[test]
    fn empty_and_short_symbols_do_not_panic() {
        assert!(!is_msvc_mutex_helper(""));
        assert!(!is_msvc_mutex_helper("x"));
    }
}
