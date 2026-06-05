//! Name-heuristic "length-companion" guesser for unsized pointer
//! parameters, plus a hardened TODO emitter.
//!
//! ## Why this lives in its own module
//!
//! Historically [`super::derive`] had a `guess_length_companion`
//! helper that returned `Option<String>` — the first sibling parameter
//! whose name looked like a length for the given pointer. That return
//! value was only ever supposed to feed a TODO comment, but the bare
//! `String` type made it trivially easy to wire into an allocation
//! decision in a future refactor (see `saw_spec_gen-c67` for the
//! design discussion: that proposal is permanently rejected because
//! the heuristic has no soundness story for ambiguous cases like
//! `void copy(void* src, void* dst, size_t n)`).
//!
//! To keep that door closed, the guesser now returns
//! [`LengthCompanionGuess`], a newtype whose only public accessor is
//! [`LengthCompanionGuess::todo_lines`]. The internal candidate list
//! is opaque to the rest of the pipeline — there is no way to
//! retrieve a single "winning" parameter name and feed it into an
//! [`crate::constraints::types::AllocType`] decision without first
//! deleting the type and recompiling the world.
//!
//! ## What changed vs. the old helper
//!
//! 1. **Ambiguity detection.** The old helper returned the first
//!    matching sibling. Ambiguous cases (`void copy(void* src,
//!    void* dst, size_t n)`) silently committed to one of them. The
//!    new helper collects *all* matches per pointer and, when more
//!    than one is plausible, the TODO calls out the ambiguity by
//!    name and explicitly refuses to guess.
//!
//! 2. **Multi-pointer awareness.** When the same sibling matches
//!    multiple pointer parameters in the same function, every match
//!    is downgraded to ambiguous — the sibling cannot bind to both.
//!
//! 3. **TODO wording.** The old text said "Heuristic: sibling
//!    parameter `n` looks like its length." which read like a soft
//!    recommendation. The new wording explicitly says the guess is
//!    name-based, not trusted, and lists the three annotation
//!    surfaces the user can reach for (Cryptol `[n][T]`, SAL
//!    annotation, sidecar `.spec.toml`).
//!
//! See the original design notes on `saw_spec_gen-dqf` and the
//! revised scope on `saw_spec_gen-c67`.

/// Outcome of running the name heuristic against a pointer parameter
/// and its siblings.
///
/// Construction is gated to [`guess`]; the only public consumer-side
/// surface is [`Self::todo_lines`], which emits a self-contained
/// block of `// TODO` comments for embedding in the generated SAW
/// script. The candidate list is intentionally not exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthCompanionGuess {
    ptr_name: String,
    /// Sibling parameter names whose lowercased form matched one of
    /// the heuristic patterns. Held in insertion order (= original
    /// parameter order) so the TODO is stable across runs.
    candidates: Vec<String>,
    /// True when the heuristic decided that *some* of the candidates
    /// match multiple pointer parameters in the same function, so the
    /// sibling cannot unambiguously bind to this pointer. Forces the
    /// "AMBIGUOUS" branch even if `candidates.len() == 1`.
    forced_ambiguous: bool,
}

impl LengthCompanionGuess {
    /// Crate-internal: copy of the candidate-name list, used by
    /// [`super::derive`] to detect cross-pointer ambiguity (the same
    /// sibling matching multiple pointers). Deliberately
    /// `pub(crate)` rather than `pub` so external code cannot pull a
    /// single "winning" name out and feed it into an alloc decision
    /// (see module docs and `saw_spec_gen-c67`).
    pub(crate) fn candidate_names_for_internal_use(&self) -> Vec<String> {
        self.candidates.clone()
    }

    /// Render the TODO comment block describing the heuristic result.
    ///
    /// Returns one of:
    ///   - "no candidate found" block (zero matches),
    ///   - "single candidate" block naming the sibling and the three
    ///     annotation surfaces (one match, not forced-ambiguous),
    ///   - "AMBIGUOUS" block listing all candidates and refusing to
    ///     guess (more than one match, or forced ambiguous).
    pub fn todo_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!(
            "// TODO[saw-spec-gen]: pointer parameter `{}` has no length annotation.",
            self.ptr_name,
        ));
        let is_ambiguous = self.forced_ambiguous || self.candidates.len() > 1;
        if self.candidates.is_empty() {
            out.push(
                "//   No length-companion sibling parameter could be guessed from names.".into(),
            );
        } else if is_ambiguous {
            let list = self
                .candidates
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!(
                "//   Name heuristic AMBIGUOUS for `{}` -- candidates: {list}.",
                self.ptr_name,
            ));
            out.push(
                "//   Heuristic refused to guess; add an explicit annotation (see below).".into(),
            );
        } else {
            // Exactly one candidate, not forced-ambiguous.
            let only = &self.candidates[0];
            out.push(format!(
                "//   Name heuristic guess: sibling `{only}` (NOT trusted -- name-based).",
            ));
        }
        // Common closing block: the three annotation surfaces.
        out.push("//   To bind a real length, use any one of (in order of preference):".into());
        out.push("//     1. Attach a Cryptol spec with `[n][T]` parameter type.".into());
        out.push("//     2. Annotate the source: `_In_reads_(n)`, `_Out_writes_(n)`,".into());
        out.push("//        `SAW_BUF(buf, n)`, or `_In_z_(MAX)`.".into());
        out.push("//     3. List the buffer in a sidecar `*.spec.toml`.".into());
        out.push(
            "//   Until then the auto-spec allocates a single element, which is almost".into(),
        );
        out.push("//   certainly wrong for any real buffer.".into());
        out
    }
}

/// Scan a pointer parameter's siblings for length-name companions and
/// build a [`LengthCompanionGuess`] describing the outcome.
///
/// The function signature is deliberately structural-only: callers
/// pass in the pointer name and the full list of parameter names.
/// The returned guess cannot leak a single "winning" sibling name
/// back into the alloc decision — see the module docs.
///
/// ### Heuristic patterns (case-insensitive)
///
/// - Bare length names: `n`, `len`, `count`, `size`, `length`, `num`.
/// - Suffix style (e.g. `buf_len`, `buf_size`): `<ptr>_len`,
///   `<ptr>_count`, `<ptr>_size`, `<ptr>_length`, `<ptr>_n`,
///   `<ptr>len`, `<ptr>count`, `<ptr>size`.
/// - Prefix / Win32 style (e.g. `nItems`, `cbBuf`, `cchBuf`,
///   `len_buf`): `n<ptr>`, `c<ptr>`, `cb<ptr>`, `cch<ptr>`,
///   `len_<ptr>`, `size_<ptr>`, `count_<ptr>`, `num_<ptr>`.
pub fn guess(ptr_name: &str, all_param_names: &[String]) -> LengthCompanionGuess {
    let candidates = collect_candidates(ptr_name, all_param_names);
    LengthCompanionGuess {
        ptr_name: ptr_name.to_string(),
        candidates,
        forced_ambiguous: false,
    }
}

/// Variant of [`guess`] used when the same sibling appears as a
/// candidate for more than one pointer parameter in the function.
/// When `shared_with_other_pointer` is true, the result is forced
/// into the "AMBIGUOUS" branch regardless of candidate count.
///
/// The derive pass scans all pointer parameters first, builds a map
/// of `sibling -> [pointers it matches]`, and sets the flag for any
/// (pointer, sibling) edge whose sibling has more than one inbound
/// pointer.
pub fn guess_with_shared_flag(
    ptr_name: &str,
    all_param_names: &[String],
    shared_with_other_pointer: bool,
) -> LengthCompanionGuess {
    let mut g = guess(ptr_name, all_param_names);
    g.forced_ambiguous = shared_with_other_pointer;
    g
}

fn collect_candidates(ptr_name: &str, all_names: &[String]) -> Vec<String> {
    let p = ptr_name.to_ascii_lowercase();
    let bare = ["n", "len", "count", "size", "length", "num"];
    let suffixes = [
        "_len", "_count", "_size", "_length", "_n", "len", "count", "size",
    ];
    let prefixes = ["n", "c", "cb", "cch", "len_", "size_", "count_", "num_"];
    let mut out = Vec::new();
    for cand in all_names {
        if cand == ptr_name {
            continue;
        }
        let c = cand.to_ascii_lowercase();
        let mut matched = false;
        if bare.contains(&c.as_str()) {
            matched = true;
        }
        if !matched {
            for s in &suffixes {
                if c == format!("{p}{s}") {
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            for pre in &prefixes {
                if c == format!("{pre}{p}") {
                    matched = true;
                    break;
                }
            }
        }
        if matched && !out.contains(cand) {
            out.push(cand.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bare_n_matches_single_pointer() {
        let g = guess("buf", &names(&["buf", "n"]));
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(
            joined.contains("Name heuristic guess: sibling `n`"),
            "{joined}"
        );
        assert!(!joined.contains("AMBIGUOUS"), "{joined}");
        assert!(
            joined.contains("Attach a Cryptol spec with `[n][T]`"),
            "{joined}",
        );
        assert!(joined.contains("SAW_BUF(buf, n)"), "{joined}");
        assert!(joined.contains("sidecar"), "{joined}");
    }

    #[test]
    fn suffix_style_buf_len() {
        let g = guess("buf", &names(&["buf", "buf_len"]));
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(joined.contains("sibling `buf_len`"), "{joined}");
        assert!(!joined.contains("AMBIGUOUS"), "{joined}");
    }

    #[test]
    fn win32_cb_prefix() {
        let g = guess("buf", &names(&["buf", "cbBuf"]));
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(joined.contains("sibling `cbBuf`"), "{joined}");
    }

    #[test]
    fn no_candidate_emits_explanatory_block() {
        let g = guess("buf", &names(&["buf", "other_ptr"]));
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(
            joined.contains("No length-companion sibling parameter could be guessed"),
            "{joined}",
        );
        // Still lists the three surfaces.
        assert!(joined.contains("[n][T]"), "{joined}");
        assert!(joined.contains("SAW_BUF"), "{joined}");
    }

    #[test]
    fn multiple_local_candidates_flagged_ambiguous() {
        // Both `n` (bare) and `buf_len` (suffix) match.
        let g = guess("buf", &names(&["buf", "n", "buf_len"]));
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(joined.contains("AMBIGUOUS"), "{joined}");
        assert!(joined.contains("`n`"), "{joined}");
        assert!(joined.contains("`buf_len`"), "{joined}");
        assert!(joined.contains("refused to guess"), "{joined}");
    }

    #[test]
    fn copy_pattern_shared_sibling_forces_ambiguous() {
        // void copy(void* src, void* dst, size_t n)
        // `n` matches both `src` and `dst`. Either guess in
        // isolation looks unambiguous; the shared-flag variant must
        // force AMBIGUOUS regardless.
        let g = guess_with_shared_flag(
            "src",
            &names(&["src", "dst", "n"]),
            /* shared_with_other_pointer = */ true,
        );
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(joined.contains("AMBIGUOUS"), "{joined}");
        assert!(joined.contains("`n`"), "{joined}");
    }

    #[test]
    fn shared_flag_no_candidates_still_lists_three_surfaces() {
        // Defensive: forced_ambiguous with empty candidates should
        // fall through the empty-candidates branch and not crash.
        let g = guess_with_shared_flag("buf", &names(&["buf", "other"]), true);
        let lines = g.todo_lines();
        let joined = lines.join("\n");
        assert!(
            joined.contains("No length-companion sibling parameter could be guessed"),
            "{joined}",
        );
        // Forced flag must not cause an AMBIGUOUS line to be emitted
        // when there are no candidates to be ambiguous between.
        assert!(!joined.contains("AMBIGUOUS"), "{joined}");
    }

    #[test]
    fn ptr_name_is_never_listed_as_its_own_companion() {
        // Avoid `void f(void* n)` matching itself as a length.
        let g = guess("n", &names(&["n"]));
        let joined = g.todo_lines().join("\n");
        assert!(
            joined.contains("No length-companion sibling parameter could be guessed"),
            "{joined}",
        );
    }
}
