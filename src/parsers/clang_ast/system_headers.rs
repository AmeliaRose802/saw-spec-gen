//! Heuristics for detecting source-location paths that correspond to
//! system/SDK headers. SAW can't easily verify inline bodies of `<cstdio>`
//! / Windows SDK functions because they call platform-specific intrinsics,
//! so we mark such functions as external.

/// Conservative: any path under a recognized vendor/SDK root counts as
/// a system include. Matching is case-insensitive and tolerates either
/// `/` or `\` as the path separator.
pub fn is_system_include_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "/microsoft visual studio/", // MSVC headers
        "/windows kits/",            // Windows SDK / UCRT
        "/program files/llvm/",
        "/program files (x86)/llvm/",
        "/clang+llvm-",
        "/include/c++/",
        "/usr/include/",
        "/usr/local/include/",
        "<built-in>",
        "<command line>",
    ];
    PATTERNS.iter().any(|pat| p.contains(pat))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_msvc_paths() {
        assert!(is_system_include_path(
            r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.40.33807\include\vector"
        ));
        assert!(is_system_include_path(
            r"C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\ucrt\stdio.h"
        ));
    }

    #[test]
    fn matches_unix_paths() {
        assert!(is_system_include_path("/usr/include/stdio.h"));
        assert!(is_system_include_path("/usr/local/include/foo.h"));
        assert!(is_system_include_path("/opt/clang+llvm-20.1.6/include/cstdio"));
        assert!(is_system_include_path("/something/include/c++/14/vector"));
    }

    #[test]
    fn matches_built_in_marker() {
        assert!(is_system_include_path("<built-in>"));
        assert!(is_system_include_path("<command line>"));
    }

    #[test]
    fn excludes_user_paths() {
        assert!(!is_system_include_path(r"C:\repos\myproj\src\foo.h"));
        assert!(!is_system_include_path("/home/me/proj/include/foo.h"));
    }
}
