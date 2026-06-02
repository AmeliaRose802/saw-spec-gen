//! STL functional override registry — issue `saw_spec_gen-jpp`.
//!
//! Crucible's LLVM memory model panics with `Cannot mux LLVM values`
//! when it tries to symbolically execute libstdc++ container code
//! (notably `std::vector::push_back` and the `std::unique_ptr`
//! allocator path). Even after `saw_spec_gen-9x5`'s `crucible_safety`
//! analyzer steers the call-graph walker away from genuinely unsafe
//! bodies, deeply nested STL helpers can still drag the walker into a
//! body that *looks* OK opcode-wise but blows up at simulation time.
//!
//! This module is the second line of defence: a hard-coded allow-list
//! of mangled-name patterns that we force the walker to treat as
//! externals, regardless of what the safety analyzer says. The
//! existing adversarial-spec emitter then havocs their return values
//! (and any `this`-pointer writes via the standard pointer-arg
//! handling), so the caller still gets a sound — if pessimistic —
//! verdict.
//!
//! The registry is consulted *before* the safety check in both
//! [`crate::gen_verify_callgraph::should_recurse`] and
//! [`crate::gen_verify_callgraph::crucible_safe`], so an override
//! match short-circuits the analyzer entirely.
//!
//! ## Kill switch
//!
//! Set `SAW_SPEC_GEN_NO_STL_OVERRIDES=1` to disable the registry. The
//! `*_gap_disproved.cpp` cases under
//! `tests/e2e/cases/10-stl-coverage/vector_back_havoc/` and
//! `unique_ptr_deref_havoc/` will then revert to the pre-`jpp`
//! Crucible panic (`UNKNOWN`).
//!
//! ## Pattern coverage
//!
//! Patterns target both Itanium (`_ZNSt...`, `_ZNKSt...`) and MSVC
//! (`?...@std@@...`) mangling. Each pattern is a `&str` checked with
//! `starts_with` against the leading portion of the mangled name *or*
//! a substring match — the matcher is intentionally permissive
//! because we'd rather over-havoc than panic.

use std::sync::OnceLock;

/// Cached read of `SAW_SPEC_GEN_NO_STL_OVERRIDES` from the
/// environment. Re-reading every call hits `getenv` on the hot path
/// (the worklist asks for every called symbol).
fn disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("SAW_SPEC_GEN_NO_STL_OVERRIDES")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// One family of STL symbols (e.g. `std::vector` methods, the
/// `unique_ptr` accessor set). Each variant lists a short description
/// for diagnostics + one-or-more matcher patterns.
struct Family {
    /// Human-readable name shown in `--verbose` output.
    name: &'static str,
    /// Substring patterns — if any appears in the mangled name, the
    /// symbol matches. Substring is used rather than `starts_with`
    /// because MSVC mangling embeds the class name in the middle of
    /// the symbol (`?back@?$vector@II@std@@...`).
    needles: &'static [&'static str],
}

const FAMILIES: &[Family] = &[
    Family {
        name: "std::vector methods",
        needles: &[
            // Itanium: _ZNSt6vector... / _ZNKSt6vector... / _ZNSt6vectorI...
            "St6vector",
            "NSt6vector",
            // MSVC: ?...@?$vector@...@std@@...
            "?$vector@",
        ],
    },
    Family {
        name: "std::unique_ptr methods",
        needles: &[
            "St10unique_ptr",
            "NSt10unique_ptr",
            "?$unique_ptr@",
            // make_unique<T>(Args&&...) — Itanium
            "St11make_uniqueI",
            "St14make_unique_for",
            // MSVC make_unique
            "?$make_unique@",
        ],
    },
    Family {
        name: "std::shared_ptr methods",
        needles: &[
            "St10shared_ptr",
            "NSt10shared_ptr",
            "?$shared_ptr@",
            "St11make_sharedI",
            "?$make_shared@",
        ],
    },
    Family {
        name: "std::basic_string methods",
        needles: &[
            // basic_string<char, char_traits, allocator>
            "Ss",
            "NSs",
            "NSbI",
            "NKSs",
            "NKSbI",
            "St12basic_string",
            "St7__cxx1112basic_string",
            // MSVC
            "?$basic_string@",
        ],
    },
    Family {
        name: "std::list / std::deque",
        needles: &[
            "St4listI",
            "NSt4list",
            "St5dequeI",
            "NSt5deque",
            "?$list@",
            "?$deque@",
        ],
    },
    Family {
        name: "std::map / std::set / std::unordered_*",
        needles: &[
            "St3mapI",
            "NSt3map",
            "St3setI",
            "NSt3set",
            "St13unordered_map",
            "St13unordered_set",
            "St8multimap",
            "St8multiset",
            "?$map@",
            "?$set@",
            "?$unordered_map@",
            "?$unordered_set@",
        ],
    },
];

/// Does any registered STL override pattern match `mangled`?
///
/// Returns `false` unconditionally when the kill-switch is set.
pub fn matches(mangled: &str) -> bool {
    if disabled() {
        return false;
    }
    matches_uncached(mangled)
}

/// Variant of [`matches`] that ignores the kill-switch. Used by unit
/// tests so they can exercise the patterns regardless of the
/// developer's local env.
pub fn matches_uncached(mangled: &str) -> bool {
    for fam in FAMILIES {
        for needle in fam.needles {
            if mangled.contains(needle) {
                return true;
            }
        }
    }
    false
}

/// Diagnostic: which family matched? Returns `None` when nothing did
/// or when the kill-switch is engaged.
pub fn family_for(mangled: &str) -> Option<&'static str> {
    if disabled() {
        return None;
    }
    for fam in FAMILIES {
        for needle in fam.needles {
            if mangled.contains(needle) {
                return Some(fam.name);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "stl_overrides_tests.rs"]
mod tests;
