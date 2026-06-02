//! Lightweight parser for Cryptol type signatures in `.cry` files.
//!
//! Extracts just enough information to detect *sret prestate threading*:
//! when a Cryptol model of a C++ aggregate-returning function takes one
//! extra trailing `[N][8]` parameter (the pre-call sret buffer bytes),
//! saw-spec-gen must emit a fresh symbolic variable for those bytes and
//! thread it into the Cryptol call.
//!
//! We parse the type signature of the named function and return the
//! decomposed parameter types and return type.

use std::path::Path;

/// A single Cryptol type in a function signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryType {
    /// `Bit`
    Bit,
    /// `[N]` — a fixed-width bitvector of `N` bits.
    Bitvector(usize),
    /// `[N][M]` — a sequence of `N` elements each `[M]` bits (byte
    /// array when M=8).
    SeqBitvector { len: usize, elem_bits: usize },
    /// Anything we don't specifically recognize.
    Other(String),
}

/// Parsed Cryptol function signature.
#[derive(Debug, Clone)]
pub struct CrySig {
    /// Input parameter types (everything before the final `-> RetType`).
    pub params: Vec<CryType>,
    /// Return type.
    pub ret: CryType,
}

impl CryType {
    /// True if this is `[N][8]` for some N — i.e. a byte-array type
    /// used for sret prestate buffers.
    pub fn is_byte_array(&self) -> bool {
        matches!(self, CryType::SeqBitvector { elem_bits: 8, .. })
    }

    /// If `[N][8]`, return N.
    pub fn byte_array_len(&self) -> Option<usize> {
        match self {
            CryType::SeqBitvector { len, elem_bits: 8 } => Some(*len),
            _ => None,
        }
    }
}

/// Detect whether the Cryptol function has exactly one extra trailing
/// `[N][8]` parameter compared to the C++ user-arg count.  Returns
/// `Some(n)` if so (the byte-array length), `None` otherwise.
///
/// `cpp_user_arg_count` is the number of C++ source-level parameters
/// (excluding the hidden sret pointer).
pub fn detect_prestate(
    sig: &CrySig,
    cpp_user_arg_count: usize,
) -> Result<Option<usize>, ArityError> {
    let cry_arity = sig.params.len();
    if cry_arity == cpp_user_arg_count {
        // Exact match — no prestate threading needed.
        Ok(None)
    } else if cry_arity == cpp_user_arg_count + 1 {
        // Candidate for prestate: extra trailing param.
        match sig.params.last() {
            Some(ty) if ty.is_byte_array() => Ok(ty.byte_array_len()),
            Some(ty) => Err(ArityError::TrailingTypeMismatch {
                cry_arity,
                cpp_arity: cpp_user_arg_count,
                trailing_type: format!("{ty:?}"),
            }),
            None => unreachable!(),
        }
    } else {
        Err(ArityError::CountMismatch {
            cry_arity,
            cpp_arity: cpp_user_arg_count,
        })
    }
}

/// Arity-related error when the Cryptol function doesn't match
/// expectations.
#[derive(Debug)]
pub enum ArityError {
    CountMismatch {
        cry_arity: usize,
        cpp_arity: usize,
    },
    TrailingTypeMismatch {
        cry_arity: usize,
        cpp_arity: usize,
        trailing_type: String,
    },
}

impl std::fmt::Display for ArityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArityError::CountMismatch {
                cry_arity,
                cpp_arity,
            } => write!(
                f,
                "Cryptol function has arity {cry_arity} but C++ ABI provides \
                 {cpp_arity} user arguments (plus the sret pointer). \
                 saw-spec-gen cannot reconcile this mismatch."
            ),
            ArityError::TrailingTypeMismatch {
                cry_arity,
                cpp_arity,
                trailing_type,
            } => write!(
                f,
                "Cryptol function has arity {cry_arity} (C++ has {cpp_arity} user args). \
                 The extra trailing parameter has type {trailing_type}, \
                 but sret prestate threading requires [N][8]."
            ),
        }
    }
}

/// Read a `.cry` file and extract the type signature of `fn_name`.
///
/// Returns `None` if the function is not found or the signature
/// cannot be parsed (we degrade gracefully — the old non-prestate
/// path will still emit, and SAW will report the partial-application
/// error if the Cryptol model truly requires prestate).
pub fn parse_signature(cry_path: &Path, fn_name: &str) -> Option<CrySig> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    parse_signature_from_str(&text, fn_name)
}

/// Parse a Cryptol function signature from source text.
pub fn parse_signature_from_str(text: &str, fn_name: &str) -> Option<CrySig> {
    // Find `fn_name :` at the start of a line (ignoring leading whitespace).
    // The colon may be on the same line or the next.
    let lines: Vec<&str> = text.lines().collect();
    let mut sig_text = String::new();

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        // Match `fn_name :` or `fn_name:` at start of trimmed line.
        if let Some(rest) = trimmed.strip_prefix(fn_name) {
            let rest = rest.trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                // Signature starts on this line after the colon.
                sig_text.push_str(after_colon.trim_start());
                // Collect continuation lines (indented or containing `->`)
                i += 1;
                while i < lines.len() {
                    let next = lines[i];
                    let next_trimmed = next.trim();
                    // Stop at blank lines or un-indented non-continuation.
                    if next_trimmed.is_empty() {
                        break;
                    }
                    // Continuation: line starts with whitespace or contains ->
                    if next.starts_with(' ') || next.starts_with('\t') {
                        sig_text.push(' ');
                        sig_text.push_str(next_trimmed);
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
        return None;
    }

    parse_arrow_type(&sig_text)
}

/// Parse a Cryptol arrow-type string like `Bit -> [16][8] -> [20][8]`.
fn parse_arrow_type(s: &str) -> Option<CrySig> {
    // Split on `->` tokens. Each segment is a type.
    let segments: Vec<&str> = s.split("->").map(|seg| seg.trim()).collect();
    if segments.len() < 2 {
        // Need at least one param and a return type.
        return None;
    }

    let mut types: Vec<CryType> = Vec::new();
    for seg in &segments {
        types.push(parse_single_type(seg));
    }

    let ret = types.pop().unwrap();
    Some(CrySig { params: types, ret })
}

/// Parse a single Cryptol type token.
fn parse_single_type(s: &str) -> CryType {
    let s = s.trim();
    if s == "Bit" {
        return CryType::Bit;
    }

    // Try `[N][M]` (sequence of bitvectors).
    if let Some(inner) = try_parse_seq_bitvector(s) {
        return inner;
    }

    // Try `[N]` (bitvector).
    if let Some(n) = try_parse_bitvector(s) {
        return CryType::Bitvector(n);
    }

    CryType::Other(s.to_string())
}

/// Try to parse `[N]` → Some(N).
fn try_parse_bitvector(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.starts_with('[') && s.ends_with(']') && !s[1..s.len() - 1].contains('[') {
        s[1..s.len() - 1].trim().parse().ok()
    } else {
        None
    }
}

/// Try to parse `[N][M]` → Some(SeqBitvector { len: N, elem_bits: M }).
fn try_parse_seq_bitvector(s: &str) -> Option<CryType> {
    let s = s.trim();
    // Match pattern [N][M] — two consecutive bracket pairs.
    if !s.starts_with('[') {
        return None;
    }
    // Find the closing bracket of the first pair.
    let first_close = s.find(']')?;
    let n_str = s[1..first_close].trim();
    let rest = &s[first_close + 1..];
    // rest should be `[M]`
    if !rest.starts_with('[') || !rest.ends_with(']') {
        return None;
    }
    let m_str = rest[1..rest.len() - 1].trim();
    let n: usize = n_str.parse().ok()?;
    let m: usize = m_str.parse().ok()?;
    Some(CryType::SeqBitvector {
        len: n,
        elem_bits: m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_bool_function() {
        let cry = "authenticate :\n  Bit -> Bit -> Bit -> Bit\n";
        let sig = parse_signature_from_str(cry, "authenticate").unwrap();
        assert_eq!(sig.params.len(), 3);
        assert!(sig.params.iter().all(|p| *p == CryType::Bit));
        assert_eq!(sig.ret, CryType::Bit);
    }

    #[test]
    fn parse_sret_prestate_function() {
        let cry = "\
getStatus_cpp :
  Bit -> Bit -> Bit -> [16][8] -> [17][8] -> [20][8]
";
        let sig = parse_signature_from_str(cry, "getStatus_cpp").unwrap();
        assert_eq!(sig.params.len(), 5);
        assert_eq!(sig.params[0], CryType::Bit);
        assert_eq!(sig.params[1], CryType::Bit);
        assert_eq!(sig.params[2], CryType::Bit);
        assert_eq!(
            sig.params[3],
            CryType::SeqBitvector {
                len: 16,
                elem_bits: 8
            }
        );
        assert_eq!(
            sig.params[4],
            CryType::SeqBitvector {
                len: 17,
                elem_bits: 8
            }
        );
        assert_eq!(
            sig.ret,
            CryType::SeqBitvector {
                len: 20,
                elem_bits: 8
            }
        );
    }

    #[test]
    fn detect_prestate_exact_match() {
        let sig = CrySig {
            params: vec![CryType::Bit, CryType::Bit],
            ret: CryType::Bitvector(32),
        };
        assert!(detect_prestate(&sig, 2).unwrap().is_none());
    }

    #[test]
    fn detect_prestate_one_extra_byte_array() {
        let sig = CrySig {
            params: vec![
                CryType::Bit,
                CryType::SeqBitvector {
                    len: 17,
                    elem_bits: 8,
                },
            ],
            ret: CryType::SeqBitvector {
                len: 20,
                elem_bits: 8,
            },
        };
        assert_eq!(detect_prestate(&sig, 1).unwrap(), Some(17));
    }

    #[test]
    fn detect_prestate_wrong_trailing_type() {
        let sig = CrySig {
            params: vec![CryType::Bit, CryType::Bitvector(64)],
            ret: CryType::Bitvector(32),
        };
        assert!(detect_prestate(&sig, 1).is_err());
    }

    #[test]
    fn detect_prestate_count_mismatch() {
        let sig = CrySig {
            params: vec![CryType::Bit, CryType::Bit, CryType::Bit],
            ret: CryType::Bitvector(32),
        };
        assert!(detect_prestate(&sig, 1).is_err());
    }

    #[test]
    fn parse_bitvector_type() {
        assert_eq!(try_parse_bitvector("[32]"), Some(32));
        assert_eq!(try_parse_bitvector("[1]"), Some(1));
        assert_eq!(try_parse_bitvector("Bit"), None);
    }

    #[test]
    fn parse_seq_bitvector_type() {
        let ty = try_parse_seq_bitvector("[16][8]").unwrap();
        assert_eq!(
            ty,
            CryType::SeqBitvector {
                len: 16,
                elem_bits: 8
            }
        );
    }

    #[test]
    fn parse_function_not_found() {
        let cry = "other_fn : Bit -> Bit\n";
        assert!(parse_signature_from_str(cry, "missing_fn").is_none());
    }

    #[test]
    fn parse_inline_signature() {
        let cry = "add_one : [32] -> [32]\n";
        let sig = parse_signature_from_str(cry, "add_one").unwrap();
        assert_eq!(sig.params.len(), 1);
        assert_eq!(sig.params[0], CryType::Bitvector(32));
        assert_eq!(sig.ret, CryType::Bitvector(32));
    }
}
