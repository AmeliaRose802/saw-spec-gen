//! Cryptol type-signature parsing helpers extracted from
//! [`super::cryptol_bridge`] to stay under the 500-NWS limit.
//!
//! These functions read `.cry` files to determine function arity,
//! parameter bit-widths, and sret pre-state detection — information
//! needed to line up SAW spec arguments with their Cryptol counterparts.

use crate::constraints::SpecConstraint;
use std::path::Path;

/// Count the arity of a Cryptol function by reading its type signature
/// from a `.cry` file. Returns `None` if the file can't be read or the
/// function's type signature isn't found.
///
/// Arity = number of `->` arrows in the type signature. A function
/// `f : A -> B -> C` has arity 2 (two parameters, return type C).
pub fn cryptol_arity(cry_path: &Path, fn_name: &str) -> Option<usize> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    cryptol_arity_from_str(&text, fn_name)
}

/// Inner implementation operating on file contents (testable without I/O).
fn cryptol_arity_from_str(text: &str, fn_name: &str) -> Option<usize> {
    let sig = collect_signature(text, fn_name)?;

    // Count top-level `->` arrows (not inside brackets/parens/braces).
    let mut arrows = 0usize;
    let mut depth = 0i32;
    let chars: Vec<char> = sig.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        match chars[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '-' if depth == 0 && i + 1 < len && chars[i + 1] == '>' => {
                arrows += 1;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    Some(arrows)
}

/// Parse `[N]` or `Bit` width from a Cryptol type fragment.
pub fn parse_cry_width(ty: &str) -> Option<u32> {
    let ty = ty.trim();
    if ty == "Bit" {
        return Some(1);
    }
    // [N][M] compound (e.g. [4][8] = 32 bits) — check BEFORE simple [N]
    if ty.starts_with('[') && !ty.ends_with(']')
        || (ty.starts_with('[') && ty.matches('[').count() > 1)
    {
        let inner = &ty[1..];
        if let Some((n_str, rest)) = inner.split_once(']') {
            if let Ok(n) = n_str.trim().parse::<u32>() {
                if let Some(m) = parse_cry_width(rest) {
                    return Some(n * m);
                }
            }
        }
    }
    // Simple [N]
    if ty.starts_with('[') && ty.ends_with(']') {
        let inner = &ty[1..ty.len() - 1];
        return inner.trim().parse().ok();
    }
    None
}

/// Read Cryptol param bit-widths from a type signature.
pub fn cryptol_param_widths(cry_path: &Path, fn_name: &str) -> Option<Vec<u32>> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    cryptol_param_widths_from_str(&text, fn_name)
}

pub(crate) fn cryptol_param_widths_from_str(text: &str, fn_name: &str) -> Option<Vec<u32>> {
    let sig = collect_signature(text, fn_name)?;
    let parts = split_top_level_arrows(&sig);
    if parts.len() <= 1 {
        return Some(vec![]);
    }
    parts[..parts.len() - 1]
        .iter()
        .map(|p| parse_cry_width(p.trim()))
        .collect()
}

/// Read the Cryptol spec's trailing parameter byte width.
pub fn cryptol_prestate_byte_width(cry_path: &Path, fn_name: &str) -> Option<usize> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    let sig = collect_signature(&text, fn_name)?;
    let parts = split_top_level_arrows(&sig);
    if parts.len() < 2 {
        return None;
    }
    let last_param = parts[parts.len() - 2].trim();
    if last_param.starts_with('[') && last_param.ends_with("[8]") {
        let inner = &last_param[1..];
        if let Some((n_str, _)) = inner.split_once(']') {
            return n_str.parse().ok();
        }
    }
    None
}

/// Detect sret pre-state threading from Cryptol arity.
///
/// When the Cryptol model has one more parameter than the C++ source-level
/// signature AND the return is sret, the extra parameter is the pre-call
/// contents of the sret buffer (for partially-initialized aggregates like
/// `std::optional<T>`).
pub fn detect_sret_prestate(
    target_spec: &mut SpecConstraint,
    cryptol_spec: &Path,
    cryptol_fn: &str,
) {
    if !target_spec.return_constraint.is_sret {
        return;
    }
    let Some(cry_arity) = cryptol_arity(cryptol_spec, cryptol_fn) else {
        return;
    };
    let cpp_param_count = target_spec.params.len();
    if cry_arity == cpp_param_count + 1 {
        eprintln!(
            "  sret pre-state detected: Cryptol {cryptol_fn} has arity {cry_arity} \
             (C++ has {cpp_param_count} params) — threading sret buffer as trailing arg",
        );
        target_spec.return_constraint.sret_prestate = true;
    } else if cry_arity > cpp_param_count + 1 {
        eprintln!(
            "warning: Cryptol {cryptol_fn} has arity {cry_arity} but C++ has \
             {cpp_param_count} source-level params (+ 1 sret). The extra {} \
             param(s) cannot be auto-threaded; hand-edit the generated spec.",
            cry_arity - cpp_param_count,
        );
    }
}

// ─── internal helpers ───────────────────────────────────────────────

/// Collect and clean a Cryptol type signature into a single string.
/// Returns `None` if the function isn't found in the text.
fn collect_signature(text: &str, fn_name: &str) -> Option<String> {
    let sig_prefix = format!("{fn_name} :");
    let mut sig_lines = Vec::new();
    let mut collecting = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !collecting {
            if trimmed.starts_with(&sig_prefix) {
                let after_colon = trimmed.split_once(':')?.1;
                sig_lines.push(after_colon.to_string());
                collecting = true;
            }
        } else {
            if trimmed.is_empty() {
                break;
            }
            let first_char = line.chars().next().unwrap_or(' ');
            if !first_char.is_whitespace() && first_char != '-' {
                break;
            }
            sig_lines.push(trimmed.to_string());
        }
    }
    if sig_lines.is_empty() {
        return None;
    }
    // Strip line comments.
    let cleaned: Vec<String> = sig_lines
        .iter()
        .map(|l| match l.find("//") {
            Some(idx) => l[..idx].to_string(),
            None => l.clone(),
        })
        .collect();
    let mut sig = cleaned.join(" ");
    // Strip block comments.
    while let Some(start) = sig.find("/*") {
        if let Some(end) = sig[start..].find("*/") {
            sig = format!("{}{}", &sig[..start], &sig[start + end + 2..]);
        } else {
            sig = sig[..start].to_string();
        }
    }
    Some(sig)
}

fn split_top_level_arrows(sig: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let chars: Vec<char> = sig.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        match chars[i] {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(chars[i]);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(chars[i]);
            }
            '-' if depth == 0 && i + 1 < len && chars[i + 1] == '>' => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 2;
                continue;
            }
            _ => current.push(chars[i]),
        }
        i += 1;
    }
    let t = current.trim().to_string();
    if !t.is_empty() {
        parts.push(t);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_arity() {
        let cry = "getStatus : Bit -> Bit -> Bit -> [16][8] -> [20][8]\n";
        assert_eq!(cryptol_arity_from_str(cry, "getStatus"), Some(4));
    }

    #[test]
    fn arity_with_prestate_param() {
        let cry = "\
getStatus :
    Bit ->
    Bit ->
    Bit ->
    [16][8] ->
    [17][8] ->
    [20][8]
";
        assert_eq!(cryptol_arity_from_str(cry, "getStatus"), Some(5));
    }

    #[test]
    fn arity_no_params() {
        let cry = "someConst : [32]\n";
        assert_eq!(cryptol_arity_from_str(cry, "someConst"), Some(0));
    }

    #[test]
    fn arity_nested_brackets() {
        let cry = "f : [8] -> ([4] -> [4]) -> [8]\n";
        assert_eq!(cryptol_arity_from_str(cry, "f"), Some(2));
    }

    #[test]
    fn arity_not_found() {
        let cry = "other : Bit -> Bit\n";
        assert_eq!(cryptol_arity_from_str(cry, "missing"), None);
    }

    #[test]
    fn arity_with_line_comment() {
        let cry = "\
getStatus :
    Bit ->  // fleetEnabled
    Bit ->  // hasKey
    [20][8]
";
        assert_eq!(cryptol_arity_from_str(cry, "getStatus"), Some(2));
    }

    #[test]
    fn cryptol_widths_simple() {
        let cry = "f : [8] -> [32]\n";
        let w = cryptol_param_widths_from_str(cry, "f").unwrap();
        assert_eq!(w, vec![8]);
    }

    #[test]
    fn cryptol_widths_multi() {
        let cry = "f : Bit -> [8] -> [32] -> [16]\n";
        let w = cryptol_param_widths_from_str(cry, "f").unwrap();
        assert_eq!(w, vec![1, 8, 32]);
    }

    #[test]
    fn cryptol_widths_byte_array() {
        let cry = "g : [4][8] -> [32]\n";
        let w = cryptol_param_widths_from_str(cry, "g").unwrap();
        assert_eq!(w, vec![32]);
    }
}
