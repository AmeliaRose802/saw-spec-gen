//! Generic string-token utilities for LLVM IR text parsing.
//!
//! These helpers don't know anything about LLVM IR semantics — they
//! just track brace / paren / angle nesting so commas and matching
//! parens can be found correctly inside structured types like
//! `{ i32, [4 x i8] }` or `sret(%struct.Foo)`.

/// Find the matching close paren for an open paren at `open_pos`.
/// Returns `None` for unbalanced input.
pub fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[open_pos..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_pos + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a comma-separated list while respecting nested `{}`, `<>`,
/// and `()` groups. Used both for parameter lists and for struct field
/// lists.
pub fn split_ir_params(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;

    for c in s.chars() {
        match c {
            '{' | '<' | '(' => {
                depth += 1;
                current.push(c);
            }
            '}' | '>' | ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

/// Update a running paren-depth counter for `token` and return the new
/// depth. Used by the attribute parsers to walk past multi-token groups
/// like `range(i32 0, 4)` where the closing `)` may live in a later
/// whitespace-separated chunk.
pub fn update_paren_depth(token: &str, depth: i32) -> i32 {
    let opens = token.chars().filter(|&c| c == '(').count() as i32;
    let closes = token.chars().filter(|&c| c == ')').count() as i32;
    depth + opens - closes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matching_paren_in_flat_input() {
        let s = "(a)b";
        assert_eq!(find_matching_paren(s, 0), Some(2));
    }

    #[test]
    fn finds_matching_paren_with_nesting() {
        let s = "f(a(b)c)d";
        assert_eq!(find_matching_paren(s, 1), Some(7));
    }

    #[test]
    fn returns_none_for_unbalanced() {
        let s = "(a";
        assert!(find_matching_paren(s, 0).is_none());
    }

    #[test]
    fn split_handles_flat_csv() {
        assert_eq!(split_ir_params("i32, i64, i8*"), vec!["i32", "i64", "i8*"],);
    }

    #[test]
    fn split_respects_braces() {
        assert_eq!(
            split_ir_params("{ i32, i64 }, i8*"),
            vec!["{ i32, i64 }", "i8*"],
        );
    }

    #[test]
    fn split_respects_parens_and_angles() {
        assert_eq!(
            split_ir_params("ptr sret(%a), <4 x i32>"),
            vec!["ptr sret(%a)", "<4 x i32>"],
        );
    }

    #[test]
    fn split_ignores_empty_pieces() {
        // Trailing/leading whitespace shouldn't yield empty entries.
        assert_eq!(split_ir_params("  i32 ,  i64  "), vec!["i32", "i64"]);
    }

    #[test]
    fn update_paren_depth_counts_both_directions() {
        assert_eq!(update_paren_depth("range(i32", 0), 1);
        assert_eq!(update_paren_depth("4)", 1), 0);
        assert_eq!(update_paren_depth("plain", 0), 0);
        assert_eq!(update_paren_depth("((", 0), 2);
        assert_eq!(update_paren_depth("))", 2), 0);
    }
}
