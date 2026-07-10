//! Low-level IR-token helpers extracted from [`super::extern_override_scan`]
//! to keep that module under the 500 NWS-line limit.

/// Split `s` on whitespace keeping bracket-enclosed spans as one token (depth only, no pair-check).
pub(super) fn split_bracket_tokens(s: &str) -> Vec<String> {
    let (mut tokens, mut current_token, mut bracket_depth) = (Vec::new(), String::new(), 0i32);
    for b in s.bytes() {
        match b {
            b'[' | b'(' | b'{' | b'<' => {
                bracket_depth += 1;
                current_token.push(b as char);
            }
            b']' | b')' | b'}' | b'>' => {
                bracket_depth -= 1;
                current_token.push(b as char);
            }
            b' ' | b'\t' if bracket_depth == 0 => {
                if !current_token.is_empty() {
                    tokens.push(std::mem::take(&mut current_token));
                }
            }
            _ => current_token.push(b as char),
        }
    }
    if !current_token.is_empty() {
        tokens.push(current_token);
    }
    tokens
}
