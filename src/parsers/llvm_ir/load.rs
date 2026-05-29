//! File I/O for LLVM IR text input, with the in-process OOM guard.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use super::function_sig::extract_functions;
use super::struct_types::struct_sizes;
use crate::constraints::FunctionInfo;

/// Maximum file size we'll attempt to load into memory (200 MB).
/// LLVM IR text is typically larger than JSON but compresses well, so
/// the limit is intentionally higher than the AST guard.
pub const MAX_IR_FILE_SIZE: u64 = 200 * 1024 * 1024;

/// Read a `.ll` file from disk. Enforces [`MAX_IR_FILE_SIZE`] before
/// attempting the read so we don't OOM on huge bitcode dumps.
pub fn parse_llvm_ir(path: &Path) -> Result<String> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_IR_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "LLVM IR file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use opt --passes=internalize,globaldce to strip unused functions\n\
             \n  2. Use llvm-extract to pull out specific functions\n\
             \n  3. Use --filter to process only matching functions after loading",
            path.display(),
            size_mb,
            MAX_IR_FILE_SIZE / (1024 * 1024),
        );
    }
    std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
}

/// Convenience helper: load optional LLVM IR text and return both the
/// struct-size table and parsed function signatures. Returns empty
/// values when `path` is `None`. Used by `gen-verify` to centralise the
/// optional-IR plumbing.
pub fn load_optional(path: Option<&Path>) -> Result<(HashMap<String, usize>, Vec<FunctionInfo>)> {
    let Some(p) = path else {
        return Ok((HashMap::new(), Vec::new()));
    };
    eprintln!("Reading LLVM IR for struct sizes: {}", p.display());
    let ir_text = parse_llvm_ir(p)?;
    let sizes = struct_sizes(&ir_text);
    eprintln!("  parsed {} struct type definitions", sizes.len());
    let funcs = extract_functions(&ir_text, None).unwrap_or_default();
    eprintln!(
        "  parsed {} function signatures (for deref sizes)",
        funcs.len()
    );
    Ok((sizes, funcs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_size_constant_is_200mb() {
        assert_eq!(MAX_IR_FILE_SIZE, 200 * 1024 * 1024);
    }

    #[test]
    fn missing_file_errors() {
        let p = std::env::temp_dir().join("does_not_exist_xyzzy.ll");
        let err = parse_llvm_ir(&p).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to stat") || msg.contains("Failed to read"));
    }

    #[test]
    fn small_file_round_trip() {
        let dir = std::env::temp_dir().join("saw_spec_gen_llvm_ir_load");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tiny.ll");
        std::fs::write(&path, "declare i32 @noop()\n").unwrap();
        let ir = parse_llvm_ir(&path).unwrap();
        assert!(ir.contains("@noop"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_optional_returns_empty_when_path_missing() {
        let (sizes, funcs) = load_optional(None).unwrap();
        assert!(sizes.is_empty());
        assert!(funcs.is_empty());
    }
}
