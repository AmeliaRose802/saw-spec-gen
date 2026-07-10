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
/// Both major C++ standard libraries lower a lock guard to a status-
/// returning C primitive that a sequential proof must treat as always
/// succeeding:
///
///   * **MSVC/UCRT** report `_Thrd_result`, whose success value
///     `_Thrd_success` is `0` (`std::scoped_lock` → `mutex::lock()` →
///     `_Mtx_lock`).
///   * **libstdc++/glibc** call the POSIX `pthread_mutex_*` primitives,
///     whose success return is likewise `0` (`std::lock_guard` →
///     `mutex::lock()` → `__gthread_mutex_lock` → `pthread_mutex_lock`).
///     The libstdc++ gthr-posix layer may reference these either
///     directly or through a `__gthrw_`-prefixed weakref alias, so the
///     prefix is stripped before matching.
///
/// A fresh-symbolic return lets the solver choose a failure code,
/// sending `mutex::lock` down its throw path (`_Throw_Cpp_error` /
/// `__throw_system_error` → `unreachable`) so the subgoal fails. Since
/// SAW verifies the *sequential* transition (concurrency is out of
/// scope), an uncontended lock/unlock/init/destroy always succeeds, so
/// pinning `0` is sound on both toolchains. This keeps the mutex e2e
/// contract identical cross-platform: the same guarded body verifies on
/// Windows and Linux, each through its own real STL primitives.
///
/// Curated, not heuristic: only the mutex families that unconditionally
/// return success in a well-formed sequential program are matched, by
/// exact symbol name. The `trylock` variants (`_Mtx_trylock`,
/// `pthread_mutex_trylock`) are intentionally excluded — each has a
/// legitimate busy return (`_Thrd_busy` / `EBUSY`) that pinning success
/// would silently erase.
pub fn success_sentinel(symbol: &str) -> Option<i64> {
    // MSVC/UCRT `_Thrd_result` mutex primitives (exact symbol match).
    const MSVC_MUTEX_PRIMITIVES: &[&str] =
        &["_Mtx_lock", "_Mtx_unlock", "_Mtx_init", "_Mtx_destroy"];
    if MSVC_MUTEX_PRIMITIVES.contains(&symbol) {
        return Some(0);
    }

    // POSIX `pthread_mutex_*` primitives (libstdc++/glibc). libstdc++
    // may call these directly or via a `__gthrw_pthread_mutex_*`
    // weakref alias, so strip that prefix before matching.
    const POSIX_MUTEX_PRIMITIVES: &[&str] = &[
        "pthread_mutex_lock",
        "pthread_mutex_unlock",
        "pthread_mutex_init",
        "pthread_mutex_destroy",
    ];
    let base = symbol.strip_prefix("__gthrw_").unwrap_or(symbol);
    if POSIX_MUTEX_PRIMITIVES.contains(&base) {
        return Some(0);
    }

    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_mutex_family() {
        for sym in ["_Mtx_lock", "_Mtx_unlock", "_Mtx_init", "_Mtx_destroy"] {
            assert_eq!(success_sentinel(sym), Some(0), "{sym} should pin 0");
        }
    }

    #[test]
    fn pins_posix_pthread_mutex_family() {
        // libstdc++/glibc lock guards call these; success == 0.
        for sym in [
            "pthread_mutex_lock",
            "pthread_mutex_unlock",
            "pthread_mutex_init",
            "pthread_mutex_destroy",
        ] {
            assert_eq!(success_sentinel(sym), Some(0), "{sym} should pin 0");
        }
    }

    #[test]
    fn pins_gthrw_weakref_pthread_mutex_family() {
        // libstdc++ gthr-posix references the primitives through a
        // `__gthrw_`-prefixed weakref alias — the prefix is stripped.
        for sym in [
            "__gthrw_pthread_mutex_lock",
            "__gthrw_pthread_mutex_unlock",
            "__gthrw_pthread_mutex_init",
            "__gthrw_pthread_mutex_destroy",
        ] {
            assert_eq!(success_sentinel(sym), Some(0), "{sym} should pin 0");
        }
    }

    #[test]
    fn trylock_is_excluded() {
        // Both toolchains: trylock has a legitimate busy return.
        assert_eq!(success_sentinel("_Mtx_trylock"), None);
        assert_eq!(success_sentinel("pthread_mutex_trylock"), None);
        assert_eq!(success_sentinel("__gthrw_pthread_mutex_trylock"), None);
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
}
