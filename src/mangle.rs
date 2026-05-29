//! Lightweight humanization of Rust v0 and Itanium mangled symbol names.
//!
//! `saw-spec-gen` is happy to use raw mangled names as SAW spec filenames
//! (they're unique by construction), but the result is unreadable:
//!
//! ```text
//! _RNvXCsdp1B4KIx7LM_11add_one_satNtB2_8ReadyU32NtNtNtCs25LAAAjbyGa_4core6future6future6Future4poll_auto_spec.saw
//! ```
//!
//! [`humanize`] extracts the human-meaningful identifier segments from
//! such a name and returns something like `ReadyU32__Future__poll`. A
//! short collision-resistant hash should be appended by the caller when
//! the result is used as an actual filename.
//!
//! Non-mangled inputs (anything not starting with `_R` or `_Z`) return
//! `None` so call-sites can fall back to existing behavior.

/// Attempt to extract a human-readable identifier from a mangled symbol.
///
/// Returns `None` if `name` is not a recognized mangled form, or if no
/// usable identifier segments could be recovered.
pub fn humanize(name: &str) -> Option<String> {
    let is_v0 = name.starts_with("_R");
    let is_itanium = name.starts_with("_Z");
    if !is_v0 && !is_itanium {
        return None;
    }

    let segs = if is_v0 {
        extract_v0_segments(name)
    } else {
        extract_itanium_segments(name)
    };

    // Itanium and v0 both emit `<digits><name>` length-prefixed segments.
    // Both use a trailing hash (Itanium: `17hd1fdb...`, v0 doesn't but
    // can have `Cs<hash>_<crate>` blocks — those still consume the crate
    // name as a real segment, so leave them in).
    let segs: Vec<&str> = segs
        .into_iter()
        .filter(|s| !is_itanium_hash(s))
        .filter(|s| !s.is_empty())
        .collect();

    if segs.is_empty() {
        return None;
    }

    // Type/trait names in Rust v0 start with an uppercase letter; method
    // and free-function names are conventionally lowercase. Keeping all
    // uppercase-leading segments plus the final identifier gives a tight
    // `Type__Trait__method` form for trait impls and `Type__method` for
    // inherent impls. Free functions (no uppercase) keep the last two
    // segments for crate context.
    let upper: Vec<&str> = segs
        .iter()
        .copied()
        .filter(|s| s.as_bytes()[0].is_ascii_uppercase())
        .collect();
    let last = *segs.last().unwrap();

    let mut chosen: Vec<&str> = if upper.is_empty() {
        if segs.len() >= 2 {
            segs[segs.len() - 2..].to_vec()
        } else {
            vec![last]
        }
    } else {
        let mut v = upper.clone();
        if v.last() != Some(&last) {
            v.push(last);
        }
        v
    };

    // For Rust v0 closures (encoded as `_RNC...`), tag the result so the
    // closure body doesn't collide with the function whose path it
    // shares (e.g. `add_one` vs `add_one`'s coroutine resume).
    if name.starts_with("_RNC") {
        chosen.push("closure");
    }

    Some(chosen.join("__"))
}

/// Walk a Rust v0 mangled name, picking out length-prefixed identifier
/// segments while skipping the base62 disambiguator / hash blocks that
/// the v0 grammar tucks between opcodes (e.g. `Cs<hash>_<crate>`,
/// `Ms<hash>_<self>`, `B<index>_`, `s<index>_`). The `_` after the
/// hash terminates each such block.
fn extract_v0_segments(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() {
            // Length-prefixed identifier: `<digits><that many bytes>`.
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let len: usize = name[i..j].parse().unwrap_or(0);
            if len == 0 {
                i = j;
                continue;
            }
            let end = j.saturating_add(len);
            if end > bytes.len() {
                i = j;
                continue;
            }
            let seg = &name[j..end];
            let first = seg.as_bytes()[0];
            if first.is_ascii_alphabetic() || first == b'_' {
                out.push(seg);
            }
            i = end;
        } else if c == b'B' || c == b's' {
            // Backref / disambiguator: base62 digits then a single `_`.
            i += 1;
            while i < bytes.len() && bytes[i] != b'_' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // consume the terminating `_`
            }
        } else {
            // Opcode letter (N, X, Y, C, M, I, Q, F, T, E, R, v, t, ...).
            // Consume one byte and continue.
            i += 1;
        }
    }
    out
}

/// Walk an Itanium mangled name. Itanium doesn't insert base62 hash
/// blocks between opcodes, so the naïve length-prefix scanner works.
fn extract_itanium_segments(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        let len: usize = name[i..j].parse().unwrap_or(0);
        if len == 0 {
            i = j;
            continue;
        }
        let end = j.saturating_add(len);
        if end > bytes.len() {
            i = j;
            continue;
        }
        let seg = &name[j..end];
        let first = seg.as_bytes()[0];
        if first.is_ascii_alphabetic() || first == b'_' {
            out.push(seg);
        }
        i = end;
    }
    out
}

/// Itanium adds a trailing `17h<16-hex>` segment to each symbol for
/// stable disambiguation. It carries no useful info — drop it.
fn is_itanium_hash(s: &str) -> bool {
    if !s.starts_with('h') || s.len() < 2 {
        return false;
    }
    s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Sanitize an arbitrary string into a filesystem-safe identifier
/// (alphanumerics and `_` only). Length is *not* bounded here — callers
/// that care about path limits should truncate the result themselves.
pub fn sanitize_filename_chars(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Short, stable hash for collision-resistant filename suffixes.
pub fn short_hash(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    let full = format!("{:016x}", h.finish());
    full[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_trait_impl_method() {
        // <ReadyU32 as Future>::poll
        let m = "_RNvXCsdp1B4KIx7LM_11add_one_satNtB2_8ReadyU32NtNtNtCs25LAAAjbyGa_4core6future6future6Future4poll";
        assert_eq!(humanize(m).as_deref(), Some("ReadyU32__Future__poll"));
    }

    #[test]
    fn v0_inherent_method() {
        // Pin::<ReadyU32>::new_unchecked
        let m = "_RNvMs4_NtCs25LAAAjbyGa_4core3pinINtB5_3PinQNtCsdp1B4KIx7LM_11add_one_sat8ReadyU32E13new_unchecked";
        assert_eq!(humanize(m).as_deref(), Some("Pin__ReadyU32__new_unchecked"));
    }

    #[test]
    fn v0_free_function() {
        let m = "_RNvCsdp1B4KIx7LM_11add_one_sat7add_one";
        assert_eq!(humanize(m).as_deref(), Some("add_one_sat__add_one"));
    }

    #[test]
    fn v0_closure() {
        // async fn add_one's coroutine resume — same path as add_one
        // but with NC closure prefix, so it must NOT collide.
        let m = "_RNCNvCsdp1B4KIx7LM_11add_one_sat7add_one0B3_";
        let h = humanize(m).unwrap();
        assert_ne!(h, "add_one_sat__add_one");
        assert!(h.ends_with("__closure"), "got: {h}");
    }

    #[test]
    fn itanium_drops_hash() {
        let m = "_ZN4core9panicking11panic_const28panic_const_async_fn_resumed17hd1fdb727465a38fdE";
        // No uppercase segments → take last 2 meaningful segments.
        let h = humanize(m).unwrap();
        assert!(h.contains("panic_const_async_fn_resumed"), "got: {h}");
        assert!(!h.contains("hd1fdb727465a38fd"), "hash leaked: {h}");
    }

    #[test]
    fn unmangled_returns_none() {
        assert_eq!(humanize("operator new"), None);
        assert_eq!(humanize("add_one"), None);
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(humanize(""), None);
        assert_eq!(humanize("_R"), None);
    }
}
