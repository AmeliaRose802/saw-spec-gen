//! Extended Cryptol signature parser that preserves type-variable
//! binders and the predicate (constraint) context.
//!
//! The original [`super::cryptol_sig`] parser was designed only for
//! sret-prestate detection: it parses concrete-length `[N][M]`
//! sequences and treats everything else as `Other(String)`. That
//! deliberately drops the `{n} (fin n, n <= 4096) =>` prefix on a
//! signature like:
//!
//! ```text
//! xor_bytes : {n}(fin n, n <= 64) => [n][8] -> [n][8] -> [n][8]
//! ```
//!
//! For the ArrayView rule "bind Cryptol [n][T] type vars to parameter
//! lengths" (`saw_spec_gen-4po`) we need the bindings, the variables
//! that appear in each parameter, and the per-variable upper bound
//! (`n <= K`) so the spec generator can allocate `llvm_array K
//! (llvm_int M)` instead of falling back to a 1-byte alloc.
//!
//! This module is a parallel parser kept deliberately small. It does
//! NOT replace [`super::cryptol_sig`] — the existing prestate path
//! continues to use the concrete parser. The two coexist on purpose:
//! prestate detection cares only about literal lengths and does not
//! need the polymorphic machinery, while length-binding only cares
//! about the polymorphic shape.

use std::collections::HashMap;
use std::path::Path;

/// One Cryptol type, with type-variable awareness.
///
/// Mirrors [`super::cryptol_sig::CryType`] but adds [`Self::SeqVar`]
/// for length-polymorphic sequences `[n][M]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyCryType {
    /// `Bit`
    Bit,
    /// `[N]` with a literal width.
    Bitvector(usize),
    /// `[N][M]` with literal length and width.
    SeqBitvector { len: usize, elem_bits: usize },
    /// `[n][M]` — sequence of `M`-bit elements indexed by Cryptol
    /// type variable `n`. The variable's upper bound (if any) is
    /// recorded separately on [`PolyCrySig::upper_bounds`].
    SeqVar { len_var: String, elem_bits: usize },
    /// Anything we don't specifically recognize.
    Other(String),
}

/// Parsed Cryptol function signature, with type-variable binders and
/// the predicate context preserved.
#[derive(Debug, Clone)]
pub struct PolyCrySig {
    /// Bound type variables, in declaration order (`{n}` -> ["n"],
    /// `{n, m}` -> ["n", "m"]).
    pub type_vars: Vec<String>,
    /// Per-type-var upper bound from a `var <= K` predicate. Missing
    /// keys mean the variable is open-ended.
    pub upper_bounds: HashMap<String, usize>,
    /// Input parameter types.
    pub params: Vec<PolyCryType>,
    /// Return type.
    pub ret: PolyCryType,
}

/// Read a `.cry` file and parse the polymorphic signature of `fn_name`.
pub fn parse_poly_signature(cry_path: &Path, fn_name: &str) -> Option<PolyCrySig> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    parse_poly_signature_from_str(&text, fn_name)
}

/// Parse a polymorphic Cryptol function signature from source text.
pub fn parse_poly_signature_from_str(text: &str, fn_name: &str) -> Option<PolyCrySig> {
    let sig_text = collect_signature_text(text, fn_name)?;
    parse_full_signature(&sig_text)
}

/// Locate `fn_name : ...` in the file and return the joined signature
/// text (continuation lines merged into one).
fn collect_signature_text(text: &str, fn_name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut sig_text = String::new();

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(rest) = trimmed.strip_prefix(fn_name) {
            let rest = rest.trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                sig_text.push_str(strip_line_comment(after_colon).trim());
                i += 1;
                while i < lines.len() {
                    let next = lines[i];
                    let next_trimmed = next.trim();
                    if next_trimmed.is_empty() {
                        break;
                    }
                    if next.starts_with(' ') || next.starts_with('\t') {
                        sig_text.push(' ');
                        sig_text.push_str(strip_line_comment(next_trimmed).trim());
                    } else {
                        break;
                    }
                    i += 1;
                }
                break;
            }
        }
        i += 1;
    }

    if sig_text.is_empty() {
        None
    } else {
        Some(sig_text)
    }
}

fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// Parse a complete signature, optionally consuming the
/// `{type_vars}(predicates) =>` prefix before the arrow-type body.
fn parse_full_signature(s: &str) -> Option<PolyCrySig> {
    let mut rest = s.trim();
    let mut type_vars: Vec<String> = Vec::new();
    let mut upper_bounds: HashMap<String, usize> = HashMap::new();

    // Optional `{a, b, c}` binder.
    if let Some(after_open) = rest.strip_prefix('{') {
        let close = after_open.find('}')?;
        let inside = &after_open[..close];
        for v in inside.split(',') {
            let name = v.trim();
            if !name.is_empty() {
                type_vars.push(name.to_string());
            }
        }
        rest = after_open[close + 1..].trim_start();
    }

    // Optional `(pred1, pred2, ...) =>` context (only when binders
    // were declared, but Cryptol allows it always; accept either).
    if let Some(after_open) = rest.strip_prefix('(') {
        let close = after_open.find(')')?;
        let inside = &after_open[..close];
        for pred in inside.split(',') {
            parse_predicate(pred.trim(), &mut upper_bounds);
        }
        rest = after_open[close + 1..].trim_start();
        // Skip the fat arrow.
        if let Some(after_fat) = rest.strip_prefix("=>") {
            rest = after_fat.trim_start();
        } else {
            // Context without `=>` is malformed; bail.
            return None;
        }
    } else if rest.starts_with("=>") {
        rest = rest.trim_start_matches("=>").trim_start();
    }

    let (params, ret) = parse_arrow_body(rest)?;
    Some(PolyCrySig {
        type_vars,
        upper_bounds,
        params,
        ret,
    })
}

/// Recognize a single Cryptol predicate. We only care about upper
/// bounds (`n <= K` / `K >= n`); everything else (`fin n`,
/// `2 ^^ n >= 16`, ...) is silently dropped — it doesn't affect
/// allocation sizing.
fn parse_predicate(pred: &str, upper_bounds: &mut HashMap<String, usize>) {
    // `n <= K`
    if let Some((lhs, rhs)) = split_once_trim(pred, "<=") {
        if let Ok(k) = rhs.parse::<usize>() {
            if is_ident(lhs) {
                upper_bounds.insert(lhs.to_string(), k);
                return;
            }
        }
        // `K <= n` form: same constraint flipped, but it's a *lower*
        // bound on `n`, so ignore for max-length purposes.
    }
    // `K >= n`
    if let Some((lhs, rhs)) = split_once_trim(pred, ">=") {
        if let Ok(k) = lhs.parse::<usize>() {
            if is_ident(rhs) {
                upper_bounds.insert(rhs.to_string(), k);
            }
        }
    }
}

fn split_once_trim<'a>(s: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let pos = s.find(sep)?;
    Some((s[..pos].trim(), s[pos + sep.len()..].trim()))
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .map(|c| c.is_alphabetic() || c == '_')
            .unwrap_or(false)
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '\'')
}

/// Parse the arrow-type body (everything after `=>`).
fn parse_arrow_body(s: &str) -> Option<(Vec<PolyCryType>, PolyCryType)> {
    let segments: Vec<&str> = s.split("->").map(|seg| seg.trim()).collect();
    if segments.len() < 2 {
        return None;
    }
    let mut types: Vec<PolyCryType> = segments.iter().map(|seg| parse_type_token(seg)).collect();
    let ret = types.pop().unwrap();
    Some((types, ret))
}

fn parse_type_token(s: &str) -> PolyCryType {
    let s = s.trim();
    if s == "Bit" {
        return PolyCryType::Bit;
    }
    if let Some(t) = try_parse_seq(s) {
        return t;
    }
    if let Some(n) = try_parse_inner_bracketed_literal(s) {
        return PolyCryType::Bitvector(n);
    }
    PolyCryType::Other(s.to_string())
}

/// Parse `[X][M]` where `X` is either a literal length or a type
/// variable identifier, and `M` is a literal element width.
fn try_parse_seq(s: &str) -> Option<PolyCryType> {
    let s = s.trim();
    if !s.starts_with('[') {
        return None;
    }
    let first_close = s.find(']')?;
    let len_str = s[1..first_close].trim();
    let rest = &s[first_close + 1..];
    if !rest.starts_with('[') || !rest.ends_with(']') {
        return None;
    }
    let m_str = rest[1..rest.len() - 1].trim();
    let m: usize = m_str.parse().ok()?;
    if let Ok(n) = len_str.parse::<usize>() {
        Some(PolyCryType::SeqBitvector {
            len: n,
            elem_bits: m,
        })
    } else if is_ident(len_str) {
        Some(PolyCryType::SeqVar {
            len_var: len_str.to_string(),
            elem_bits: m,
        })
    } else {
        None
    }
}

fn try_parse_inner_bracketed_literal(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.starts_with('[') && s.ends_with(']') && !s[1..s.len() - 1].contains('[') {
        s[1..s.len() - 1].trim().parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_no_binder_concrete_signature() {
        let sig = parse_poly_signature_from_str("k : [16][8] -> [32]", "k").unwrap();
        assert!(sig.type_vars.is_empty());
        assert!(sig.upper_bounds.is_empty());
        assert_eq!(
            sig.params,
            vec![PolyCryType::SeqBitvector {
                len: 16,
                elem_bits: 8
            }],
        );
        assert_eq!(sig.ret, PolyCryType::Bitvector(32));
    }

    #[test]
    fn parses_single_type_var_with_upper_bound() {
        let cry = "xor_bytes : {n}(fin n, n <= 64) => [n][8] -> [n][8] -> [n][8]";
        let sig = parse_poly_signature_from_str(cry, "xor_bytes").unwrap();
        assert_eq!(sig.type_vars, vec!["n"]);
        assert_eq!(sig.upper_bounds.get("n"), Some(&64));
        assert_eq!(sig.params.len(), 2);
        for p in &sig.params {
            assert_eq!(
                p,
                &PolyCryType::SeqVar {
                    len_var: "n".into(),
                    elem_bits: 8
                }
            );
        }
        assert_eq!(
            sig.ret,
            PolyCryType::SeqVar {
                len_var: "n".into(),
                elem_bits: 8
            }
        );
    }

    #[test]
    fn parses_multiple_type_vars_with_bounds() {
        let cry = "h : {n, m}(fin n, fin m, n <= 64, m <= 16) => [n][32] -> [m][8] -> [32]";
        let sig = parse_poly_signature_from_str(cry, "h").unwrap();
        assert_eq!(sig.type_vars, vec!["n", "m"]);
        assert_eq!(sig.upper_bounds.get("n"), Some(&64));
        assert_eq!(sig.upper_bounds.get("m"), Some(&16));
        assert_eq!(sig.params.len(), 2);
        assert_eq!(
            sig.params[0],
            PolyCryType::SeqVar {
                len_var: "n".into(),
                elem_bits: 32
            }
        );
        assert_eq!(
            sig.params[1],
            PolyCryType::SeqVar {
                len_var: "m".into(),
                elem_bits: 8
            }
        );
    }

    #[test]
    fn open_ended_type_var_has_no_bound() {
        // `{n} (fin n) =>` — no upper bound. Polymorphic but no MAX.
        let cry = "f : {n}(fin n) => [n][8] -> [32]";
        let sig = parse_poly_signature_from_str(cry, "f").unwrap();
        assert_eq!(sig.type_vars, vec!["n"]);
        assert!(sig.upper_bounds.is_empty());
        assert_eq!(
            sig.params,
            vec![PolyCryType::SeqVar {
                len_var: "n".into(),
                elem_bits: 8
            }]
        );
    }

    #[test]
    fn accepts_k_geq_n_form() {
        // `64 >= n` is the same constraint as `n <= 64`.
        let cry = "f : {n}(fin n, 64 >= n) => [n][8] -> [32]";
        let sig = parse_poly_signature_from_str(cry, "f").unwrap();
        assert_eq!(sig.upper_bounds.get("n"), Some(&64));
    }

    #[test]
    fn missing_function_returns_none() {
        let cry = "f : Bit -> Bit";
        assert!(parse_poly_signature_from_str(cry, "g").is_none());
    }

    #[test]
    fn multiline_signature_is_collected() {
        let cry = "\
xor_bytes :
  {n} (fin n, n <= 64) =>
  [n][8] -> [n][8] -> [n][8]
";
        let sig = parse_poly_signature_from_str(cry, "xor_bytes").unwrap();
        assert_eq!(sig.type_vars, vec!["n"]);
        assert_eq!(sig.upper_bounds.get("n"), Some(&64));
        assert_eq!(sig.params.len(), 2);
    }

    #[test]
    fn bare_bit_param() {
        let sig = parse_poly_signature_from_str("g : Bit -> Bit -> Bit", "g").unwrap();
        assert_eq!(sig.params, vec![PolyCryType::Bit, PolyCryType::Bit]);
        assert_eq!(sig.ret, PolyCryType::Bit);
    }
}
