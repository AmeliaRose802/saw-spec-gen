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
/// MSVC/UCRT mutex primitives report `_Thrd_result`, whose success
/// value `_Thrd_success` is `0`. In a lock-guarded body these land in
/// the override set on every path (`std::scoped_lock` → `mutex::lock()`
/// → `_Mtx_lock`), and a fresh-symbolic return lets the solver choose a
/// failure code, sending `_Mutex_base::lock` down its `_Throw_Cpp_error`
/// → `unreachable` path so the subgoal fails. Since SAW verifies the
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
    const SUCCESS_SENTINEL_PRIMITIVES: &[&str] =
        &["_Mtx_lock", "_Mtx_unlock", "_Mtx_init", "_Mtx_destroy"];
    SUCCESS_SENTINEL_PRIMITIVES.contains(&symbol).then_some(0)
}

/// Substring patterns that identify MSVC `_Mutex_base` internal helpers.
///
/// These functions are defined in-module with `linkonce_odr` linkage (not
/// `declare`-only) and perform typed reads on `_Mutex_base` fields
/// (ownership/level counts). When the mutex struct is modeled as flat
/// symbolic bytes, these reads cause "Error during memory load" under SAW
/// symbolic execution. In a sequential proof the mutex is always
/// uncontended, so abstracting them as no-ops returning a success value
/// is sound.
const MSVC_MUTEX_HELPER_PATTERNS: &[&str] = &[
    // `std::_Mutex_base::_Verify_ownership_levels` — MSVC-mangled as
    // `?_Verify_ownership_levels@_Mutex_base@std@@...`
    "_Verify_ownership_levels",
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
/// `bool`-returning `_Verify_ownership_levels`.
pub fn msvc_mutex_noop_return(symbol: &str) -> Option<i64> {
    // `_Verify_ownership_levels` returns `bool` (i1). In a sequential proof
    // the mutex ownership levels are always valid — pin `true` (1).
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
    fn trylock_is_excluded() {
        assert_eq!(success_sentinel("_Mtx_trylock"), None);
    }

    #[test]
    fn ordinary_symbol_is_not_pinned() {
        assert_eq!(success_sentinel("compute_checksum"), None);
        assert_eq!(success_sentinel("printf"), None);
    }

    #[test]
    fn verify_ownership_levels_is_msvc_mutex_helper() {
        // Exact MSVC-mangled name for `std::_Mutex_base::_Verify_ownership_levels`.
        assert!(is_msvc_mutex_helper(
            "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"
        ));
        // Plain unmangled name also matches (used in unit tests).
        assert!(is_msvc_mutex_helper("_Verify_ownership_levels"));
    }

    #[test]
    fn unrelated_symbols_are_not_msvc_mutex_helpers() {
        assert!(!is_msvc_mutex_helper("_Mtx_lock"));
        assert!(!is_msvc_mutex_helper("compute_checksum"));
        assert!(!is_msvc_mutex_helper("printf"));
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
