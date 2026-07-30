//! Shared knowledge about C-runtime "status" primitives whose assumed
//! overrides must pin a success sentinel instead of a fresh-symbolic
//! return.
//!
//! Used by both override emitters:
//!   * [`super::bitcode_overrides`] for symbols that only appear in the
//!     bitcode (reached via inlined STL bodies the clang AST never
//!     exposes as direct calls), and
//!   * [`super::llvm_spec::generate_unspecified_spec`] for the same
//!     symbols when they *are* visible in the AST as declared-only
//!     externals (e.g. `<mutex>` pulls in a prototype for `_Mtx_lock`).
//!
//! Keeping the curated list in one place means both paths agree on
//! exactly which symbols are pinned and to what value.

/// Success-sentinel return value for a known threading status
/// primitive, or `None` for any other symbol.
///
/// MSVC/UCRT mutex primitives and Linux pthread/gthread mutex lock and
/// unlock primitives return `0` on success. In a lock-guarded body these
/// land in the override set on every path (`std::scoped_lock` →
/// `mutex::lock()`), and a fresh-symbolic return lets the solver choose
/// a failure code, sending the implementation down an exception/
/// `unreachable` path so the subgoal fails. Since SAW verifies the
/// *sequential* transition (concurrency is out of scope), an
/// uncontended lock/unlock/init/destroy always succeeds, so pinning `0`
/// is sound for these.
///
/// Curated, not heuristic: only the mutex family that unconditionally
/// returns `_Thrd_success` in a well-formed sequential program is
/// matched, by exact symbol name. `_Mtx_trylock` is intentionally
/// excluded — it has a legitimate `_Thrd_busy` return that pinning
/// success would silently erase.
pub fn success_sentinel(symbol: &str) -> Option<i64> {
    const SUCCESS_SENTINEL_PRIMITIVES: &[&str] = &[
        "_Mtx_lock",
        "_Mtx_unlock",
        "_Mtx_init",
        "_Mtx_destroy",
        "__gthread_mutex_lock",
        "__gthread_mutex_unlock",
        "pthread_mutex_lock",
        "pthread_mutex_unlock",
    ];
    SUCCESS_SENTINEL_PRIMITIVES.contains(&symbol).then_some(0)
}

/// Substring patterns that identify MSVC `std::_Mutex_base` internal helpers.
///
/// These functions are defined in-module with `linkonce_odr` linkage (not
/// `declare`-only) and perform typed reads on `_Mutex_base` fields
/// (ownership/level counts). When the mutex struct is modeled as flat
/// symbolic bytes, these reads cause "Error during memory load" under SAW
/// symbolic execution. In a sequential proof the mutex is always
/// uncontended, so abstracting them as no-ops returning a success value
/// is sound.
///
/// The pattern `_Mutex_base@std` appears in every MSVC-mangled method of
/// `std::_Mutex_base` (e.g. `?_Verify_ownership_levels@_Mutex_base@std@@...`,
/// `?_Stl_critical_section_begin@_Mutex_base@std@@...`), so it future-proofs
/// detection of all sibling helpers without an explicit per-method list.
const MSVC_MUTEX_HELPER_PATTERNS: &[&str] = &[
    // Matches any method of `std::_Mutex_base` in MSVC-mangled form.
    // MSVC encodes class and namespace as `@ClassName@Namespace@`, so every
    // method of `std::_Mutex_base` contains this exact substring.
    "_Mutex_base@std",
];

/// Whether `symbol` (mangled or unmangled) is a known MSVC `_Mutex_base`
/// internal helper that should be overridden as a no-op in sequential proofs.
pub fn is_msvc_mutex_helper(symbol: &str) -> bool {
    MSVC_MUTEX_HELPER_PATTERNS
        .iter()
        .any(|pat| symbol.contains(pat))
}

/// No-op return value to pin for a known MSVC mutex helper, or `None` for
/// any other symbol.  The value is an integer suitable for
/// `llvm_term {{ <val> : [<bits>] }}` — e.g. `1` (true) for the
/// `bool`-returning helpers like `_Verify_ownership_levels`.
pub fn msvc_mutex_noop_return(symbol: &str) -> Option<i64> {
    // All `std::_Mutex_base` ownership helpers return `bool` (i1). In a
    // sequential proof the mutex is always uncontended and ownership levels
    // are always valid — pin `true` (1).
    if is_msvc_mutex_helper(symbol) {
        Some(1)
    } else {
        None
    }
}

/// Whether `symbol` is a C++ runtime throw helper that is `[[noreturn]]`
/// by contract — it always throws (or re-throws / aborts) and never
/// returns to its caller.
///
/// The compiler emits an `unreachable` immediately after every call to
/// such a helper. A default override that "returns" lets SAW fall into
/// that `unreachable`, failing the proof spuriously (e.g. the MSVC
/// `std::scoped_lock` path calls `_Throw_Cpp_error` on a — sequentially
/// impossible — lock failure). Emitting a `False` post-condition on the
/// assumed override instead models the noreturn contract: the caller
/// assumes the call never returns, so the following `unreachable` is
/// provably dead. Sound because every listed symbol is genuinely
/// `[[noreturn]]`.
///
/// Matched by substring so both MSVC (`?_Xlength_error@std@@...`) and
/// Itanium (`_ZSt20__throw_length_errorPKc`) manglings are covered.
pub fn is_noreturn_throw(symbol: &str) -> bool {
    const NORETURN_THROW_PATTERNS: &[&str] = &[
        // MSVC: low-level throw + the `std::_X*` throw-helper family.
        "_Throw_Cpp_error",
        "_CxxThrowException",
        "_Xlength_error",
        "_Xout_of_range",
        "_Xbad_alloc",
        "_Xinvalid_argument",
        "_Xruntime_error",
        "_Xoverflow_error",
        "_Xbad_function_call",
        "_Xbad_optional_access",
        // Itanium / libstdc++: `__cxa_throw`, `__cxa_rethrow`, and the
        // whole `std::__throw_*` family all contain `__throw` / `__cxa`.
        "__cxa_throw",
        "__cxa_rethrow",
        "__throw_",
    ];
    NORETURN_THROW_PATTERNS
        .iter()
        .any(|pat| symbol.contains(pat))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_mutex_family() {
        for sym in [
            "_Mtx_lock",
            "_Mtx_unlock",
            "_Mtx_init",
            "_Mtx_destroy",
            "__gthread_mutex_lock",
            "__gthread_mutex_unlock",
            "pthread_mutex_lock",
            "pthread_mutex_unlock",
        ] {
            assert_eq!(success_sentinel(sym), Some(0), "{sym} should pin 0");
        }
    }

    #[test]
    fn trylock_is_excluded() {
        assert_eq!(success_sentinel("_Mtx_trylock"), None);
    }

    #[test]
    fn ordinary_symbol_is_not_pinned() {
        assert_eq!(success_sentinel("compute_checksum"), None);
        assert_eq!(success_sentinel("printf"), None);
    }

    #[test]
    fn msvc_mutex_base_methods_are_helpers() {
        // `_Mutex_base@std` appears in every MSVC-mangled method of
        // `std::_Mutex_base` — verify a few representative names.
        assert!(is_msvc_mutex_helper(
            "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"
        ));
        // A hypothetical sibling method is also caught without a dedicated
        // pattern entry — this is the future-proofing the change provides.
        assert!(is_msvc_mutex_helper(
            "?_Stl_critical_section_begin@_Mutex_base@std@@IEAAXXZ"
        ));
    }

    #[test]
    fn unrelated_symbols_are_not_msvc_mutex_helpers() {
        assert!(!is_msvc_mutex_helper("_Mtx_lock"));
        assert!(!is_msvc_mutex_helper("compute_checksum"));
        assert!(!is_msvc_mutex_helper("printf"));
        // GCC-mangled name for a simulation struct does NOT match — the
        // pattern is MSVC-specific (`@ClassName@Namespace@` encoding).
        assert!(!is_msvc_mutex_helper(
            "_ZN15_Mutex_base_sim24_Verify_ownership_levelsEv"
        ));
    }

    #[test]
    fn msvc_mutex_helper_noop_return_pins_true() {
        assert_eq!(
            msvc_mutex_noop_return("?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"),
            Some(1)
        );
        assert_eq!(msvc_mutex_noop_return("_Mtx_lock"), None);
        assert_eq!(msvc_mutex_noop_return("compute_checksum"), None);
    }

    #[test]
    fn recognizes_noreturn_throw_helpers() {
        for sym in [
            "?_Throw_Cpp_error@std@@YAXH@Z",
            "?_Xlength_error@std@@YAXPEBD@Z",
            "?_Xout_of_range@std@@YAXPEBD@Z",
            "_CxxThrowException",
            "__cxa_throw",
            "__cxa_rethrow",
            "_ZSt20__throw_length_errorPKc",
            "_ZSt24__throw_bad_optional_accessv",
        ] {
            assert!(is_noreturn_throw(sym), "{sym} should be noreturn-throw");
        }
    }

    #[test]
    fn non_throw_symbols_are_not_noreturn() {
        assert!(!is_noreturn_throw("_Mtx_lock"));
        assert!(!is_noreturn_throw("memcmp"));
        assert!(!is_noreturn_throw("compute_checksum"));
    }
}
