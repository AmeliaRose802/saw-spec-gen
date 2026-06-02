//! Cryptol/LLVM type bridge helpers shared across SAW-spec emitters.
//!
//! Both the C++ generator ([`super::verify_script_steps`]) and the
//! Rust generator ([`crate::gen_verify_rust`]) must agree on how each
//! LLVM-side parameter is presented to a Cryptol spec. Centralizing
//! the conversion in one module means a `*_spec.cry` file authored for
//! one runner type-checks on the other without surprise.
//!
//! The single non-trivial case today is `bool` ↔ `Bit`: C++ `bool` and
//! Rust `bool` both lower to LLVM `i1`, which SAW exposes as the
//! Cryptol sequence type `[1]`. Cryptol's primitive boolean type is
//! `Bit` though, and idiomatic specs declare boolean parameters as
//! `Bit -> …` so `\/`, `/\`, `~`, etc. work directly. The `(name ! 0)`
//! wrap extracts bit 0 from the `[1]` and yields a `Bit`.
//!
//! Add new bridges here when a future type (e.g. `f32`/`f64`) needs
//! similar adjustment — never re-implement the convention in a single
//! runner only.

use crate::constraints::TypeInfo;

/// Whether `ty` is a `bool` (or pointer/reference to `bool`) as seen
/// by the front-end parsers. Both lower to LLVM `i1` at the call
/// boundary and therefore need Bit/`[1]` bridging on the Cryptol side.
pub fn is_bool_like(ty: &TypeInfo) -> bool {
    match ty {
        TypeInfo::Bool => true,
        TypeInfo::Pointer(inner) => matches!(inner.as_ref(), TypeInfo::Bool),
        _ => false,
    }
}

/// Bridge a Cryptol-side argument expression to match the LLVM-side
/// representation expected by the C++/Rust call. C++/Rust `bool`
/// parameters become `(name ! 0)` so the Cryptol callsite sees a
/// `Bit`. Other types pass through unchanged.
pub fn cryptol_arg_for(name: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("({name} ! 0)")
    } else {
        name.to_string()
    }
}

/// Bridge a Cryptol-side call expression to match the LLVM-side return
/// representation produced by the C++/Rust function. A `Bit`-returning
/// Cryptol spec is wrapped as `[…] : [1]` so it lines up with the
/// LLVM `i1` returned by a C++/Rust `bool`. Other types pass through.
pub fn cryptol_return_for(call: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("[{call}] : [1]")
    } else {
        call.to_string()
    }
}

/// Format a counterexample literal value as a typed Cryptol expression.
/// For `bits == 1` inputs, bridges via `((v : [1]) ! 0)` so the call
/// reaches a `Bit`-typed parameter. Returns plain `(v : [bits])` for
/// every other width.
///
/// Used by counterexample evaluators that need to call the same
/// Cryptol spec on a concrete witness produced by SAW.
pub fn cryptol_literal_for_bits(value: u64, bits: u32) -> String {
    if bits == 1 {
        format!("(({value} : [1]) ! 0)")
    } else {
        format!("({value} : [{bits}])")
    }
}

/// Count the arity of a Cryptol function by reading its type signature
/// from a `.cry` file. Returns `None` if the file can't be read or the
/// function's type signature isn't found.
///
/// Arity = number of `->` arrows in the type signature. A function
/// `f : A -> B -> C` has arity 2 (two parameters, return type C).
pub fn cryptol_arity(cry_path: &std::path::Path, fn_name: &str) -> Option<usize> {
    let text = std::fs::read_to_string(cry_path).ok()?;
    cryptol_arity_from_str(&text, fn_name)
}

/// Inner implementation operating on file contents (testable without I/O).
fn cryptol_arity_from_str(text: &str, fn_name: &str) -> Option<usize> {
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
    // Strip line comments before joining — each line piece may have
    // its own trailing `// …` that must be removed independently.
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

#[cfg(test)]
mod arity_tests {
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
        // Two top-level arrows: [8] -> (… -> …) -> [8]
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
}

/// Detect sret pre-state threading from Cryptol arity.
///
/// When the Cryptol model has one more parameter than the C++ source-level
/// signature AND the return is sret, the extra parameter is the pre-call
/// contents of the sret buffer (for partially-initialized aggregates like
/// `std::optional<T>`).
pub fn detect_sret_prestate(
    target_spec: &mut crate::constraints::SpecConstraint,
    cryptol_spec: &std::path::Path,
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
