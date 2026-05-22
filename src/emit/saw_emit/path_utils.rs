//! Lightweight relative-path computation used by the verification
//! script emitter to produce portable, repo-relative paths.

use std::path::{Path, PathBuf};

/// Compute a forward-slash relative path from `base` to `target`, or
/// fall back to the original (lossy) path string when canonicalization
/// fails. The result is always usable in a SAW `include`/`import`
/// directive on any platform.
pub fn pathdiff_or_absolute(target: &Path, base: &Path) -> String {
    if let (Ok(target_abs), Ok(base_abs)) = (target.canonicalize(), base.canonicalize()) {
        if let Some(rel) = pathdiff(&target_abs, &base_abs) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    target.to_string_lossy().replace('\\', "/").to_string()
}

/// Compute a relative path from `base` to `path` by skipping the common
/// component prefix and prepending `..` for each remaining `base`
/// component. Returns `None` when either path is empty (which the
/// callable patterns avoid in practice).
pub fn pathdiff(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut path_components = path.components().peekable();
    let mut base_components = base.components().peekable();

    // Skip common prefix.
    while let (Some(p), Some(b)) = (path_components.peek(), base_components.peek()) {
        if p == b {
            path_components.next();
            base_components.next();
        } else {
            break;
        }
    }

    let mut result = PathBuf::new();
    for _ in base_components {
        result.push("..");
    }
    for c in path_components {
        result.push(c);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pathdiff_skips_common_prefix() {
        let p = Path::new("/a/b/c/file.txt");
        let b = Path::new("/a/b/d");
        let rel = pathdiff(p, b).unwrap();
        assert_eq!(rel, PathBuf::from("../c/file.txt"));
    }

    #[test]
    fn pathdiff_handles_identical_paths() {
        let p = Path::new("/a/b");
        let rel = pathdiff(p, p).unwrap();
        assert_eq!(rel, PathBuf::new());
    }

    #[test]
    fn pathdiff_target_under_base() {
        let p = Path::new("/a/b/c/d");
        let b = Path::new("/a/b");
        let rel = pathdiff(p, b).unwrap();
        assert_eq!(rel, PathBuf::from("c/d"));
    }

    #[test]
    fn pathdiff_or_absolute_returns_forward_slashes() {
        let p = Path::new(r"a\b\c.txt");
        let out = pathdiff_or_absolute(p, Path::new("."));
        assert!(!out.contains('\\'), "got {out}");
    }

    #[test]
    fn pathdiff_disjoint_paths_walk_back() {
        let p = Path::new("/a/b/c");
        let b = Path::new("/x/y/z");
        let rel = pathdiff(p, b).unwrap();
        assert_eq!(rel, PathBuf::from("../../../a/b/c"));
    }
}
