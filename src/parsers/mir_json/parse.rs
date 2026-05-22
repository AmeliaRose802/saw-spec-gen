//! File loading + size guard for mir-json input.
//!
//! Uses [`serde_json::from_reader`] backed by a `BufReader<File>` so
//! the on-disk JSON is consumed in a streaming fashion rather than
//! materialised as a single `String` ahead of parsing. Peak memory is
//! therefore dominated by the parsed [`MirJson`] graph, not by 2× the
//! file size like the previous `read_to_string` pipeline.

use super::node::MirJson;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Sanity ceiling on mir-json input file size (2 GiB).
///
/// Streaming makes the on-disk byte count a poor proxy for peak memory,
/// so this is a guardrail against pathological inputs (e.g. accidentally
/// pointed at a multi-GB log) rather than a memory cap. Raise via
/// `SAW_SPEC_GEN_MAX_MIR_BYTES` for very large crates.
pub const MAX_MIR_FILE_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Environment override for [`MAX_MIR_FILE_SIZE`] (raw byte count).
const MAX_MIR_FILE_SIZE_ENV: &str = "SAW_SPEC_GEN_MAX_MIR_BYTES";

/// Buffer size for the streaming file reader (1 MiB).
const READ_BUFFER_BYTES: usize = 1024 * 1024;

fn effective_max_mir_size() -> u64 {
    std::env::var(MAX_MIR_FILE_SIZE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(MAX_MIR_FILE_SIZE)
}

/// Read a mir-json output file from disk and decode it into a typed
/// [`MirJson`]. Streams via a buffered reader so the source bytes are
/// never held in a `String` alongside the parsed structure.
pub fn parse_mir(path: &Path) -> Result<MirJson> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    let max_size = effective_max_mir_size();
    if file_size > max_size {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "MIR file {} is too large ({} MB, sanity limit is {} MB). \
             Streaming reduces peak memory but cannot bound the parsed \
             MIR graph itself — raise the limit via {}=<bytes> if you \
             really need to load this file. Otherwise, try:\n\
             \n  1. Use --filter to process only specific functions\n\
             \n  2. Build with a smaller crate scope (e.g. single lib target)\n\
             \n  3. Pre-filter with: jq '.fns |= map(select(.name | test(\"pattern\")))' large.json > filtered.json",
            path.display(),
            size_mb,
            max_size / (1024 * 1024),
            MAX_MIR_FILE_SIZE_ENV,
        );
    }

    let file = File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    serde_json::from_reader(reader)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_size_constant_is_2gib() {
        assert_eq!(MAX_MIR_FILE_SIZE, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn missing_file_errors_with_stat_message() {
        let p = std::env::temp_dir().join("no_such_mir_file.json");
        let err = parse_mir(&p).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Failed to stat")
                || msg.contains("Failed to read")
                || msg.contains("Failed to open")
        );
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
