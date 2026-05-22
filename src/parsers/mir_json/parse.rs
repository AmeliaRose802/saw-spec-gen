//! File loading + size guard for mir-json input.

use super::node::MirJson;
use anyhow::{Context, Result};
use std::path::Path;

/// Maximum input file size that [`parse_mir`] will load (100 MB).
/// mir-json output for large crates can balloon quickly; the error
/// message guides users toward filtering.
pub const MAX_MIR_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Read a mir-json output file from disk and decode it into a typed
/// [`MirJson`]. Enforces [`MAX_MIR_FILE_SIZE`] before reading so we
/// don't OOM on huge multi-crate dumps.
pub fn parse_mir(path: &Path) -> Result<MirJson> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_MIR_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "MIR file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use --filter to process only specific functions\n\
             \n  2. Build with a smaller crate scope (e.g. single lib target)\n\
             \n  3. Pre-filter with: jq '.fns |= map(select(.name | test(\"pattern\")))' large.json > filtered.json",
            path.display(),
            size_mb,
            MAX_MIR_FILE_SIZE / (1024 * 1024),
        );
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_size_constant_is_100mb() {
        assert_eq!(MAX_MIR_FILE_SIZE, 100 * 1024 * 1024);
    }

    #[test]
    fn missing_file_errors_with_stat_message() {
        let p = std::env::temp_dir().join("no_such_mir_file.json");
        let err = parse_mir(&p).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to stat") || msg.contains("Failed to read"));
    }

    #[test]
    fn small_file_round_trip() {
        let dir = std::env::temp_dir().join("saw_spec_gen_mir_parse");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tiny.json");
        std::fs::write(&path, r#"{"fns":[],"adts":[]}"#).unwrap();
        let mir = parse_mir(&path).unwrap();
        assert!(mir.fns.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
