//! Process-wide cache of source-file contents, used to recover SAL macro
//! names dropped by clang's JSON AST dumper.
//!
//! Background: `clang -ast-dump=json` emits the *kind* of every
//! `AnnotateAttr` but, since clang 17, drops the string argument that
//! the text dumper preserves. To recover macro names like `_Out_` we
//! read the original source file at the attribute's `expansionLoc`
//! byte range and extract the literal token. Caching avoids reading
//! the same `.cpp` or `.h` file once per annotation.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read `len` bytes starting at byte `offset` from `path`. Returns
/// `None` if the file is unreadable, the slice is out of range, or the
/// bytes aren't valid UTF-8. File contents are cached for the lifetime
/// of the process.
pub fn read_source_token(path: &str, offset: usize, len: usize) -> Option<String> {
    let mut guard = cache().lock().ok()?;
    let contents = if let Some(c) = guard.get(path) {
        c.clone()
    } else {
        let c = std::fs::read_to_string(path).ok()?;
        guard.insert(path.to_string(), c.clone());
        c
    };
    let bytes = contents.as_bytes();
    let end = offset.checked_add(len)?;
    if end > bytes.len() {
        return None;
    }
    std::str::from_utf8(&bytes[offset..end])
        .ok()
        .map(|s| s.to_string())
}

/// Read a SAL-style macro invocation token plus any immediately
/// following `(...)` argument list.
///
/// clang's `expansionLoc.tokLen` only covers the macro *name*
/// (`_In_reads_`); the size argument `(8)` lives in the source after
/// the recorded token. To recover the full text we read the base token
/// at the recorded length and, if it's followed by `(`, continue
/// scanning until the matching `)`. Nesting and string literals are
/// handled conservatively (single-char parens, no escapes -- SAL never
/// embeds either).
///
/// Returns `None` only when even the base token can't be read. If the
/// base reads successfully but no `(` follows, returns just the base.
pub fn read_source_macro_with_args(
    path: &str,
    offset: usize,
    base_len: usize,
) -> Option<String> {
    let base = read_source_token(path, offset, base_len)?;
    // Grab a generous window after the base token and scan it for a
    // balanced `(...)` group. 128 bytes is well past anything SAL
    // emits (the longest realistic form is `_Out_writes_bytes_(N)`
    // with N a few digits).
    let mut guard = cache().lock().ok()?;
    let contents = guard
        .entry(path.to_string())
        .or_insert_with(|| std::fs::read_to_string(path).unwrap_or_default());
    let bytes = contents.as_bytes();
    let after_base = offset.checked_add(base_len)?;
    if after_base >= bytes.len() || bytes[after_base] != b'(' {
        return Some(base);
    }
    let scan_end = (after_base + 128).min(bytes.len());
    let mut depth = 0usize;
    let mut close_at = None;
    for i in after_base..scan_end {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close_at = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = close_at?;
    std::str::from_utf8(&bytes[offset..end])
        .ok()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_a_token_from_a_real_file() {
        let dir = std::env::temp_dir().join("saw_spec_gen_src_cache");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sample.h");
        std::fs::write(&path, b"void f(_In_ int x);").unwrap();
        let s = read_source_token(path.to_str().unwrap(), 7, 4).unwrap();
        assert_eq!(s, "_In_");
        // Second read hits the cache (file mutation should NOT be observed)
        std::fs::write(&path, b"trash").unwrap();
        let s2 = read_source_token(path.to_str().unwrap(), 7, 4).unwrap();
        assert_eq!(s2, "_In_");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn out_of_range_returns_none() {
        let dir = std::env::temp_dir().join("saw_spec_gen_src_cache_oor");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("short.h");
        std::fs::write(&path, b"abc").unwrap();
        assert!(read_source_token(path.to_str().unwrap(), 100, 5).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(read_source_token("nope/not/here.h", 0, 1).is_none());
    }
}
