//! Typed accumulator for LLVM IR parameter attributes.
//!
//! Replaces the legacy "ten parallel `bool` locals + two depth
//! counters" state machine in `parse_ir_param_resolved`. Each
//! attribute LLVM may attach to a parameter maps to exactly one field
//! here, and the token classifier returns an enum variant — no more
//! repeated `if part == "readonly" { is_readonly = true; }` matchers.

/// Token classification result for a single whitespace-separated
/// attribute token (or attribute-group fragment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrToken {
    /// `readonly` — pointer is read-only.
    Readonly,
    /// `writeonly` — pointer is write-only.
    Writeonly,
    /// `noalias` — pointer doesn't alias other params.
    Noalias,
    /// `nocapture` — pointer value isn't stored.
    Nocapture,
    /// `nonnull` — pointer is non-null.
    Nonnull,
    /// `sret` or `sret(%struct.Foo)` (fully balanced in one token).
    /// Carries the optional inner struct type for type resolution.
    Sret(Option<String>),
    /// `sret([24` — the opening fragment of a `sret([N x T])` token
    /// whose inner type contains spaces and spans multiple whitespace-
    /// separated tokens.  Carries the partial inner text and the
    /// current unmatched open-paren depth (always ≥ 1).
    SretStart(String, i32),
    /// `dereferenceable(N)` — known-good byte count behind the pointer.
    Dereferenceable(usize),
    /// `align` — the next token holds the integer alignment.
    AlignKeyword,
    /// One of the recognized single-word attributes that we simply skip
    /// (e.g. `zeroext`, `mustprogress`, `memory(none)`, …).
    SkipSingle,
    /// Opening of a parenthesized attribute group whose contents may
    /// span multiple whitespace-separated tokens (e.g. `range(i32 0,`
    /// followed by `4)`). The carried `depth_delta` tells the caller
    /// how much paren depth opens net of any closes already in this
    /// token.
    SkipParenGroupStart { depth_delta: i32 },
    /// A `%name` / `@name` identifier following a type — the parameter
    /// name. Numeric names (`%0`) are renamed to `arg0` for SAW.
    Name(String),
    /// Anything else — treat as a fragment of the type string.
    TypeFragment,
}

impl IrToken {
    /// Classify a single whitespace-separated token. Caller is
    /// responsible for resetting any `SkipParenGroupStart` depth
    /// counter once it reaches zero.
    pub fn classify(part: &str, type_so_far: &str) -> Self {
        match part {
            "readonly" => return IrToken::Readonly,
            "writeonly" => return IrToken::Writeonly,
            "noalias" => return IrToken::Noalias,
            "nocapture" => return IrToken::Nocapture,
            "nonnull" => return IrToken::Nonnull,
            "align" => return IrToken::AlignKeyword,
            _ => {}
        }
        if SKIP_SINGLE_ATTRS.contains(&part) {
            return IrToken::SkipSingle;
        }
        if let Some(rest) = part.strip_prefix("sret(") {
            // Track paren depth across `rest`.  We start at 1 because the
            // opening '(' of "sret(" is already consumed by strip_prefix —
            // it counts as an open that still needs to be closed.
            let mut depth = 1i32;
            for ch in rest.chars() {
                match ch {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            if depth == 0 {
                // Fully balanced in one token: sret(typename)
                let inner = rest.strip_suffix(')').unwrap_or(rest);
                return IrToken::Sret(Some(inner.to_string()));
            } else {
                // Multi-token: sret([24 x i8]) — inner type has spaces.
                // `rest` is everything after "sret(", depth > 0 means how
                // many unmatched opens remain across subsequent tokens.
                return IrToken::SretStart(rest.to_string(), depth);
            }
        }
        if part == "sret" {
            return IrToken::Sret(None);
        }
        if let Some(rest) = part.strip_prefix("dereferenceable(") {
            if let Ok(n) = rest.trim_end_matches(')').parse() {
                return IrToken::Dereferenceable(n);
            }
        }
        if is_paren_attr_prefix(part) {
            let opens = part.chars().filter(|&c| c == '(').count() as i32;
            let closes = part.chars().filter(|&c| c == ')').count() as i32;
            return IrToken::SkipParenGroupStart {
                depth_delta: opens - closes,
            };
        }
        if SKIP_SINGLE_KEYWORDS.contains(&part) || part.starts_with('#') {
            return IrToken::SkipSingle;
        }
        // A `%`/`@` token that comes after a non-empty type string is a
        // parameter name. If we haven't accumulated a type yet, treat
        // it as part of the type instead (rare but possible for
        // forward-referenced types).
        if (part.starts_with('%') || part.starts_with('@'))
            && !part.ends_with('*')
            && !type_so_far.is_empty()
        {
            return IrToken::Name(normalize_param_name(part));
        }
        IrToken::TypeFragment
    }
}

/// Typed accumulator of every attribute we extract from a parameter's
/// token stream. The `type_str` and `name` fields hold the partially
/// reconstructed type and parameter name as we walk the token list.
#[derive(Debug, Default)]
pub struct IrParamAttrs {
    pub readonly: bool,
    pub writeonly: bool,
    pub noalias: bool,
    pub nocapture: bool,
    pub nonnull: bool,
    pub sret: bool,
    pub deref_size: Option<usize>,
    pub type_str: String,
    pub name: Option<String>,
}

impl IrParamAttrs {
    /// Walk the token list, classifying each token and updating the
    /// accumulator. Handles the multi-token state ([`AlignKeyword`]
    /// consumes the next token; [`SkipParenGroupStart`] consumes
    /// tokens until paren depth returns to zero).
    ///
    /// [`AlignKeyword`]: IrToken::AlignKeyword
    /// [`SkipParenGroupStart`]: IrToken::SkipParenGroupStart
    pub fn from_parts(parts: &[&str]) -> Self {
        let mut attrs = IrParamAttrs::default();
        let mut skip_next = false;
        let mut paren_depth: i32 = 0;
        // State for multi-token `sret([N x T])` where the inner type
        // contains spaces.  `sret_depth > 0` means we're still
        // accumulating the inner type string.
        let mut sret_accum = String::new();
        let mut sret_depth: i32 = 0;

        for part in parts {
            if skip_next {
                skip_next = false;
                continue;
            }
            // Accumulate the remainder of a multi-token sret inner type.
            if sret_depth > 0 {
                let opens = part.chars().filter(|&c| c == '(').count() as i32;
                let closes = part.chars().filter(|&c| c == ')').count() as i32;
                sret_depth += opens - closes;
                if sret_depth <= 0 {
                    // This token closes the sret(…) group; strip its
                    // trailing ')' (which belongs to the sret wrapper).
                    sret_accum.push(' ');
                    sret_accum.push_str(part.strip_suffix(')').unwrap_or(part));
                    attrs.type_str = std::mem::take(&mut sret_accum);
                    sret_depth = 0;
                } else {
                    sret_accum.push(' ');
                    sret_accum.push_str(part);
                }
                continue;
            }
            if paren_depth > 0 {
                paren_depth += paren_depth_delta(part);
                continue;
            }
            match IrToken::classify(part, &attrs.type_str) {
                IrToken::Readonly => attrs.readonly = true,
                IrToken::Writeonly => attrs.writeonly = true,
                IrToken::Noalias => attrs.noalias = true,
                IrToken::Nocapture => attrs.nocapture = true,
                IrToken::Nonnull => attrs.nonnull = true,
                IrToken::AlignKeyword => skip_next = true,
                IrToken::Sret(inner) => {
                    attrs.sret = true;
                    if let Some(ty) = inner {
                        attrs.type_str = ty;
                    }
                }
                IrToken::SretStart(partial, depth) => {
                    attrs.sret = true;
                    sret_accum = partial;
                    sret_depth = depth;
                }
                IrToken::Dereferenceable(n) => attrs.deref_size = Some(n),
                IrToken::SkipSingle => {}
                IrToken::SkipParenGroupStart { depth_delta } => {
                    paren_depth = depth_delta.max(0);
                }
                IrToken::Name(n) => attrs.name = Some(n),
                IrToken::TypeFragment => {
                    if !attrs.type_str.is_empty() {
                        attrs.type_str.push(' ');
                    }
                    attrs.type_str.push_str(part);
                }
            }
        }
        attrs
    }
}

fn paren_depth_delta(part: &str) -> i32 {
    let opens = part.chars().filter(|&c| c == '(').count() as i32;
    let closes = part.chars().filter(|&c| c == ')').count() as i32;
    opens - closes
}

/// Convert a raw `%name` / `@name` identifier into a SAW-friendly
/// parameter name. LLVM allows numeric names like `%0`; SAW identifiers
/// must start with a letter, so prefix `arg` when the first character
/// is a digit.
fn normalize_param_name(part: &str) -> String {
    let raw = part.trim_start_matches('%').trim_start_matches('@');
    if raw.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("arg{raw}")
    } else {
        raw.to_string()
    }
}

/// True when `token` introduces a parenthesized attribute group whose
/// body may span subsequent whitespace-separated tokens (e.g.
/// `range(i32 0,` continues into `4)`).
fn is_paren_attr_prefix(token: &str) -> bool {
    PAREN_ATTR_PREFIXES.iter().any(|p| token.starts_with(p))
}

/// Single-token attributes that LLVM IR places on parameters but that
/// the SAW emitter doesn't care about. Listed alphabetically.
const SKIP_SINGLE_ATTRS: &[&str] = &[
    "captures(none)",
    "dead_on_unwind",
    "immarg",
    "inreg",
    "memory(argmem:",
    "memory(none)",
    "memory(read)",
    "memory(write)",
    "mustprogress",
    "nofree",
    "nosync",
    "noundef",
    "nounwind",
    "returned",
    "signext",
    "willreturn",
    "writable",
    "zeroext",
];

/// Function-level attributes that may appear in the param list and
/// should be ignored.
const SKIP_SINGLE_KEYWORDS: &[&str] = &[
    "alwaysinline",
    "cold",
    "dllexport",
    "dllimport",
    "hot",
    "noinline",
    "noreturn",
    "optnone",
    "optsize",
    "uwtable",
];

/// Token prefixes that introduce parenthesized attribute groups whose
/// closing `)` may live in a later whitespace-separated token.
const PAREN_ATTR_PREFIXES: &[&str] = &[
    "align(",
    "byref(",
    "byval(",
    "captures(",
    "dereferenceable_or_null(",
    "inalloca(",
    "initializes(",
    "memory(",
    "preallocated(",
    "range(",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> IrParamAttrs {
        let parts: Vec<&str> = s.split_whitespace().collect();
        IrParamAttrs::from_parts(&parts)
    }

    #[test]
    fn picks_up_basic_mutability_flags() {
        let a = parse("i32* readonly noalias");
        assert!(a.readonly);
        assert!(a.noalias);
        assert!(!a.writeonly);
        assert_eq!(a.type_str, "i32*");
    }

    #[test]
    fn sret_with_inner_struct_seeds_type_str() {
        let a = parse("ptr sret(%struct.Foo) noalias %retval");
        assert!(a.sret);
        assert_eq!(a.type_str, "%struct.Foo");
        assert_eq!(a.name.as_deref(), Some("retval"));
    }

    #[test]
    fn sret_bare_marks_flag_without_seeding_type() {
        let a = parse("ptr sret %retval");
        assert!(a.sret);
        assert_eq!(a.type_str, "ptr");
        assert_eq!(a.name.as_deref(), Some("retval"));
    }

    #[test]
    fn dereferenceable_picks_up_count() {
        let a = parse("ptr dereferenceable(32)");
        assert_eq!(a.deref_size, Some(32));
    }

    #[test]
    fn align_keyword_consumes_next_token() {
        let a = parse("ptr align 8 %x");
        assert_eq!(a.name.as_deref(), Some("x"));
        // The "8" must not leak into the type.
        assert!(!a.type_str.contains('8'));
    }

    #[test]
    fn paren_group_spans_multiple_tokens() {
        // `range(i32 0, 4)` spreads across 3 whitespace-separated tokens
        let a = parse("i32 range(i32 0, 4) %x");
        // The type accumulator must not include any fragment of the
        // range attribute group.
        assert_eq!(a.type_str, "i32");
        assert_eq!(a.name.as_deref(), Some("x"));
    }

    #[test]
    fn nested_paren_groups_balance() {
        let a = parse("ptr range(i32 (0), (4)) %x");
        assert_eq!(a.type_str, "ptr");
        assert_eq!(a.name.as_deref(), Some("x"));
    }

    #[test]
    fn initializes_attr_does_not_leak_into_sret_type() {
        // Clang emits `initializes((0, N))` on sret slots (LLVM 20+).
        // It must be skipped as a parenthesized attribute group; if it
        // leaked into the type string it would corrupt the pointee
        // struct name and defeat the sret byte-size fallback (see
        // docs/20-optional-sret-e2e-post-pr74-regression.md).
        let a = parse(
            "ptr dead_on_unwind noalias nocapture writable writeonly \
             sret(%\"class.std::optional\") align 1 initializes((0, 17)) %0",
        );
        assert!(a.sret);
        assert_eq!(a.type_str, "%\"class.std::optional\"");
        assert_eq!(a.name.as_deref(), Some("arg0"));
    }

    #[test]
    fn initializes_attr_with_multiple_ranges_is_skipped() {
        let a = parse("ptr sret(%struct.Foo) initializes((0, 4), (8, 12)) %r");
        assert!(a.sret);
        assert_eq!(a.type_str, "%struct.Foo");
        assert_eq!(a.name.as_deref(), Some("r"));
    }

    #[test]
    fn numeric_param_names_get_arg_prefix() {
        let a = parse("i32 %0");
        assert_eq!(a.name.as_deref(), Some("arg0"));
    }

    #[test]
    fn pointer_type_token_is_treated_as_type_not_name() {
        // `i8*` ends with `*` so should not be classified as a name.
        let a = parse("i8* readonly");
        assert_eq!(a.type_str, "i8*");
        assert!(a.name.is_none());
    }

    #[test]
    fn skip_attrs_dont_leak_into_type() {
        let a = parse("ptr nounwind willreturn zeroext noundef");
        assert_eq!(a.type_str, "ptr");
    }

    #[test]
    fn classify_recognizes_align_keyword() {
        assert_eq!(IrToken::classify("align", ""), IrToken::AlignKeyword);
    }

    #[test]
    fn classify_recognizes_dereferenceable_with_count() {
        assert_eq!(
            IrToken::classify("dereferenceable(64)", ""),
            IrToken::Dereferenceable(64),
        );
    }

    #[test]
    fn classify_recognizes_sret_variants() {
        assert_eq!(IrToken::classify("sret", ""), IrToken::Sret(None));
        match IrToken::classify("sret(%S)", "") {
            IrToken::Sret(Some(s)) => assert_eq!(s, "%S"),
            other => panic!("expected Sret(Some), got {other:?}"),
        }
    }

    #[test]
    fn classify_name_requires_type_already_present() {
        // No type yet → token is treated as a type fragment.
        assert_eq!(IrToken::classify("%x", ""), IrToken::TypeFragment);
        // With type present → it's a name.
        match IrToken::classify("%x", "i32") {
            IrToken::Name(n) => assert_eq!(n, "x"),
            other => panic!("expected Name, got {other:?}"),
        }
    }
}
