//! Shared constants and a small infallible writer wrapper used by the
//! various spec-emission modules.
//!
//! The original `saw_emit.rs` reached for `out.push_str(&format!(...))`
//! several hundred times. That works, but it allocates a temporary
//! `String` per write and obscures the intent of the code. The
//! [`SpecWriter`] newtype wraps a `String` and exposes the standard
//! `std::fmt::Write` based macros (`write!`, `writeln!`) without
//! forcing every call site to import the trait and `.unwrap()` the
//! infallible `fmt::Result`. New code should prefer the [`line!`] and
//! [`blank!`] helpers; existing untouched code keeps its current style.

#![allow(unused_macros, unused_imports)]

use std::fmt::Write as _;

/// SAW type sentinel emitted for `void`-returning functions. We use a
/// comment prefix so the value is harmless if it ever leaks into a
/// generated SAW script verbatim.
pub const VOID_SAW_TYPE: &str = "// void";

/// Standard 4-space indent used inside SAW `do` blocks.
#[allow(dead_code)]
pub const INDENT: &str = "    ";

/// Returns `true` when a SAW type string represents `void` — i.e. the
/// function has no return value to bind. Centralizing this check keeps
/// the spec-emitters honest about what counts as a void return.
pub fn is_void_saw_type(saw_type: &str) -> bool {
    saw_type == VOID_SAW_TYPE
}

/// Newtype around `String` that exposes infallible `write!` / `writeln!`
/// helpers. The standard `fmt::Write` implementation on `String` never
/// returns `Err`, so swallowing the `fmt::Result` here is sound.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct SpecWriter {
    buf: String,
}

#[allow(dead_code)]
impl SpecWriter {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: String::with_capacity(cap),
        }
    }

    /// Consume the writer and return the underlying `String`.
    pub fn into_string(self) -> String {
        self.buf
    }

    /// Append a pre-formatted string verbatim.
    pub fn push(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    /// Append a blank line.
    pub fn blank(&mut self) {
        self.buf.push('\n');
    }

    /// Append an indented line with no formatting. Equivalent to
    /// `{INDENT}{s}\n`.
    pub fn indented(&mut self, s: &str) {
        self.buf.push_str(INDENT);
        self.buf.push_str(s);
        self.buf.push('\n');
    }

    /// Borrow the underlying buffer (read-only).
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// Borrow the underlying buffer as a mutable `String`, for callers
    /// that still need direct `push_str` / `write!` access.
    pub fn as_mut_string(&mut self) -> &mut String {
        &mut self.buf
    }
}

impl std::fmt::Write for SpecWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.buf.write_str(s)
    }
}

/// Helper to format a SAW comment-style header line.
#[allow(dead_code)]
pub fn comment_line(text: &str) -> String {
    format!("// {text}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn void_constant_is_stable() {
        assert_eq!(VOID_SAW_TYPE, "// void");
        assert!(is_void_saw_type("// void"));
        assert!(!is_void_saw_type("llvm_int 32"));
        assert!(!is_void_saw_type("// VOID")); // case-sensitive
    }

    #[test]
    fn writer_supports_writeln_macro() {
        let mut w = SpecWriter::new();
        writeln!(w, "hello {}", "world").unwrap();
        assert_eq!(w.as_str(), "hello world\n");
    }

    #[test]
    fn writer_helpers_compose() {
        let mut w = SpecWriter::with_capacity(64);
        w.push("// header\n");
        w.blank();
        w.indented("body");
        assert_eq!(w.as_str(), "// header\n\n    body\n");
    }

    #[test]
    fn into_string_returns_buffer() {
        let mut w = SpecWriter::new();
        w.push("done");
        assert_eq!(w.into_string(), "done");
    }

    #[test]
    fn indent_constant_is_four_spaces() {
        assert_eq!(INDENT, "    ");
        assert_eq!(INDENT.len(), 4);
    }

    #[test]
    fn comment_line_includes_trailing_newline() {
        assert_eq!(comment_line("foo"), "// foo\n");
    }
}
