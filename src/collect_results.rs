//! `collect-results`: aggregate per-run `result.json` files into a
//! single `proof_manifest.json` for downstream consumers
//! (currently `pretty-specs --proof-status`).
//!
//! Inputs are produced by `verify.ps1`, `verify-rust.ps1`, and
//! `verify-equiv.ps1` via `scripts/Write-ResultJson.ps1`.  The
//! on-disk shape they conform to is documented in
//! `docs/result-json.md` — schema version `"1"` is the only revision
//! this collector accepts; anything else is rejected with a clear
//! error so consumers can't silently consume incompatible payloads.
//!
//! The output manifest deliberately stays minimal: one `flat` array
//! of entries today, with `structured` reserved for the closed-loop
//! integration described in `bd show saw-spec-gen-3eq`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Schema versions of `result.json` this collector accepts.  Adding a
/// new entry here is the migration mechanism when the producer shape
/// changes — see `docs/result-json.md`.
const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["1"];

/// Output format selection from the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormat {
    /// Single flat array of entries; the only shape pretty-specs
    /// consumes today (see `pretty-specs-2oa`).
    Flat,
    /// Reserved for the grouped shape pretty-specs-ua2 will introduce.
    Structured,
}

impl ManifestFormat {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "flat" => Ok(ManifestFormat::Flat),
            "structured" => Ok(ManifestFormat::Structured),
            other => bail!("unknown --format value '{other}' (expected: flat | structured)"),
        }
    }
}

/// One per-run record on disk, as produced by `Write-ResultJson.ps1`.
/// Only the fields this collector actually consumes are deserialised;
/// unknown fields are tolerated so newer producers can add metadata
/// without breaking older consumers.
#[derive(Debug, Clone, Deserialize)]
struct RawResult {
    /// Defaults to the pseudo-version `"0"` when the producer omitted
    /// the field (pre-schema-v1 layouts).  The supported-versions
    /// check below turns that into a clear, actionable error rather
    /// than letting `serde` surface a `missing field` message.
    #[serde(default = "missing_schema_version")]
    schema_version: String,
    side: String,
    function: String,
    cryptol_fn: String,
    verdict: String,
    #[serde(default)]
    counterexample: Vec<RawCounterexample>,
    #[serde(default)]
    impl_file: Option<String>,
    #[serde(default)]
    solver: Option<String>,
    #[serde(default)]
    time_secs: Option<f64>,
}

fn missing_schema_version() -> String {
    "<missing>".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct RawCounterexample {
    name: String,
    value: Option<String>,
    #[serde(default)]
    bits: Option<u32>,
}

/// One manifest entry — keyed by `function` (the implementation
/// function name, or `cryptol_fn` when an `--cryptol-fn-map` alias
/// applies so pretty-specs can match by `Item::Function.name`).
#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntry {
    pub key: String,
    pub function: String,
    pub cryptol_fn: String,
    pub side: String,
    /// Mapped status: `proven` / `failed` / `not_attempted`.
    pub status: &'static str,
    pub verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Path the `result.json` was loaded from, relative to the
    /// `--root` directory.  Useful for follow-up debugging.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impl_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_secs: Option<f64>,
}

/// Top-level manifest written to `--output`.
#[derive(Debug, Serialize)]
pub struct ProofManifest {
    pub schema_version: &'static str,
    pub generator: &'static str,
    pub entries: Vec<ManifestEntry>,
}

const MANIFEST_SCHEMA_VERSION: &str = "1";
const GENERATOR_TAG: &str = "saw-spec-gen collect-results";

/// Entry point for the `collect-results` subcommand.
pub fn run(
    root: &Path,
    output: &Path,
    cryptol_fn_map: Option<&Path>,
    format: ManifestFormat,
) -> Result<()> {
    if !root.is_dir() {
        bail!("--root must be a directory: {}", root.display());
    }

    let alias_map = load_alias_map(cryptol_fn_map)?;

    let mut result_files = Vec::new();
    collect_result_files(root, &mut result_files)?;
    result_files.sort();

    eprintln!(
        "Scanning {}: found {} result.json file(s)",
        root.display(),
        result_files.len(),
    );

    let mut entries = Vec::with_capacity(result_files.len());
    for path in &result_files {
        let entry = read_entry(path, root, &alias_map)
            .with_context(|| format!("failed to read {}", path.display()))?;
        entries.push(entry);
    }

    // Stable ordering by (key, side, source) makes diffs across CI runs
    // readable; pretty-specs only cares that every key is present once.
    entries.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then_with(|| a.side.cmp(&b.side))
            .then_with(|| a.source.cmp(&b.source))
    });

    write_manifest(output, &entries, format)?;
    eprintln!("Wrote {} entries to {}", entries.len(), output.display());
    Ok(())
}

fn load_alias_map(path: Option<&Path>) -> Result<HashMap<String, String>> {
    let Some(p) = path else {
        return Ok(HashMap::new());
    };
    let text = std::fs::read_to_string(p)
        .with_context(|| format!("failed to read --cryptol-fn-map {}", p.display()))?;
    let parsed: HashMap<String, String> = serde_json::from_str(&text).with_context(|| {
        format!(
            "--cryptol-fn-map {} is not a JSON object of string→string",
            p.display()
        )
    })?;
    Ok(parsed)
}

fn collect_result_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // Skip well-known noise (target/, node_modules/, .git/).
            // The verify scripts only ever land artifacts under
            // out_*/, so users hit fewer surprises with a broad walk.
            if should_skip_dir(&path) {
                continue;
            }
            collect_result_files(&path, out)?;
        } else if file_type.is_file() && path.file_name() == Some(OsStr::new("result.json")) {
            out.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    matches!(
        name,
        "target" | "node_modules" | ".git" | ".beads" | "dist" | "build"
    )
}

fn read_entry(
    path: &Path,
    root: &Path,
    alias_map: &HashMap<String, String>,
) -> Result<ManifestEntry> {
    let text = std::fs::read_to_string(path)?;
    let raw: RawResult = serde_json::from_str(&text)
        .with_context(|| "result.json is not valid JSON or is missing required fields")?;

    if !SUPPORTED_SCHEMA_VERSIONS.contains(&raw.schema_version.as_str()) {
        return Err(anyhow!(
            "unsupported result.json schema_version '{}' in {} \
             (supported: {}). \
             Bump saw-spec-gen or regenerate the result.json with a \
             compatible verify script.",
            raw.schema_version,
            path.display(),
            SUPPORTED_SCHEMA_VERSIONS.join(", "),
        ));
    }

    let (status, reason) = map_verdict(&raw);

    let source = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    // Alias map: applied to `function` so the manifest key can match
    // pretty-specs' Cryptol-side `Item::Function.name`.  When no alias
    // is present we just key by the implementation function name.
    let key = alias_map
        .get(&raw.function)
        .cloned()
        .unwrap_or_else(|| raw.function.clone());

    Ok(ManifestEntry {
        key,
        function: raw.function.clone(),
        cryptol_fn: raw.cryptol_fn,
        side: raw.side,
        status,
        verdict: raw.verdict,
        reason,
        source,
        impl_file: raw.impl_file,
        solver: raw.solver,
        time_secs: raw.time_secs,
    })
}

fn map_verdict(raw: &RawResult) -> (&'static str, Option<String>) {
    match raw.verdict.as_str() {
        "VERIFIED" | "EQUIVALENT" => ("proven", None),
        "DISPROVED" | "NOT EQUIVALENT" => ("failed", Some(format_counterexample_reason(raw))),
        "UNKNOWN" => ("not_attempted", Some("SAW returned UNKNOWN".into())),
        other => (
            "not_attempted",
            Some(format!("unrecognised verdict: {other}")),
        ),
    }
}

fn format_counterexample_reason(raw: &RawResult) -> String {
    if raw.counterexample.is_empty() {
        return "disproved (no counterexample captured)".into();
    }
    let bindings: Vec<String> = raw
        .counterexample
        .iter()
        .map(|c| {
            let val = c.value.as_deref().unwrap_or("?");
            match c.bits {
                Some(b) => format!("{} = {} (i{b})", c.name, val),
                None => format!("{} = {}", c.name, val),
            }
        })
        .collect();
    format!("counterexample: {}", bindings.join(", "))
}

fn write_manifest(output: &Path, entries: &[ManifestEntry], format: ManifestFormat) -> Result<()> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let manifest = ProofManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        generator: GENERATOR_TAG,
        entries: entries.to_vec(),
    };
    let json = match format {
        ManifestFormat::Flat => serde_json::to_string_pretty(&manifest)?,
        ManifestFormat::Structured => {
            // Structured groups entries by `key`.  Until pretty-specs-ua2
            // pins the grouped shape we keep it deliberately simple:
            // { key: [entry, entry, ...], ... }.  Each value is a list
            // so multiple sides (cpp / rust / equiv) for the same key
            // round-trip without one shadowing another.
            let mut grouped: std::collections::BTreeMap<&str, Vec<&ManifestEntry>> =
                std::collections::BTreeMap::new();
            for e in entries {
                grouped.entry(e.key.as_str()).or_default().push(e);
            }
            let body = serde_json::json!({
                "schema_version": MANIFEST_SCHEMA_VERSION,
                "generator": GENERATOR_TAG,
                "groups": grouped,
            });
            serde_json::to_string_pretty(&body)?
        }
    };
    std::fs::write(output, json + "\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempdir_compat::TempDir;

    #[test]
    fn parses_verified_result() {
        let dir = TempDir::new("collect-verified").unwrap();
        let p = dir.path().join("result.json");
        fs::write(
            &p,
            r#"{
                "schema_version": "1",
                "side": "cpp",
                "function": "add_one",
                "cryptol_fn": "add_one_spec",
                "verdict": "VERIFIED",
                "counterexample": [],
                "expected": null,
                "actual": null,
                "solver": "z3",
                "time_secs": null,
                "impl_file": "add_one.cpp"
            }"#,
        )
        .unwrap();
        let out = dir.path().join("manifest.json");
        run(dir.path(), &out, None, ManifestFormat::Flat).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        let entries = manifest["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["status"], "proven");
        assert_eq!(entries[0]["function"], "add_one");
        assert_eq!(entries[0]["key"], "add_one");
    }

    #[test]
    fn maps_disproved_to_failed_with_reason() {
        let dir = TempDir::new("collect-disproved").unwrap();
        let p = dir.path().join("out_a").join("result.json");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            r#"{
                "schema_version": "1",
                "side": "rust",
                "function": "f",
                "cryptol_fn": "f_spec",
                "verdict": "DISPROVED",
                "counterexample": [{ "name": "x", "value": "7", "bits": 32 }],
                "expected": "8",
                "actual": "0",
                "solver": "z3",
                "time_secs": null,
                "impl_file": "f.rs"
            }"#,
        )
        .unwrap();
        let out = dir.path().join("manifest.json");
        run(dir.path(), &out, None, ManifestFormat::Flat).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        let entry = &manifest["entries"][0];
        assert_eq!(entry["status"], "failed");
        let reason = entry["reason"].as_str().unwrap();
        assert!(reason.contains("x = 7"), "got {reason:?}");
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let dir = TempDir::new("collect-bad-schema").unwrap();
        let p = dir.path().join("result.json");
        fs::write(
            &p,
            r#"{
                "schema_version": "99",
                "side": "cpp",
                "function": "f",
                "cryptol_fn": "f_spec",
                "verdict": "VERIFIED"
            }"#,
        )
        .unwrap();
        let out = dir.path().join("manifest.json");
        let err = run(dir.path(), &out, None, ManifestFormat::Flat).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("schema_version"), "got: {msg}");
        assert!(msg.contains("'99'"), "got: {msg}");
    }

    #[test]
    fn cryptol_fn_map_remaps_key() {
        let dir = TempDir::new("collect-alias").unwrap();
        let map_path = dir.path().join("aliases.json");
        fs::write(&map_path, r#"{"impl_add": "spec_add"}"#).unwrap();
        let p = dir.path().join("result.json");
        fs::write(
            &p,
            r#"{
                "schema_version": "1",
                "side": "cpp",
                "function": "impl_add",
                "cryptol_fn": "add_one_spec",
                "verdict": "VERIFIED"
            }"#,
        )
        .unwrap();
        let out = dir.path().join("manifest.json");
        run(dir.path(), &out, Some(&map_path), ManifestFormat::Flat).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        // Manifest will also contain a stray entry for aliases.json being
        // walked? No — aliases.json file name != result.json so it's
        // skipped.  But the script walker reads only files named
        // result.json so only one entry shows up.
        let entries = manifest["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["key"], "spec_add");
        assert_eq!(entries[0]["function"], "impl_add");
    }

    // Minimal in-tree temp directory shim — avoids adding `tempfile` as a
    // dev dependency just for these tests.  Cleans up on Drop.
    mod tempdir_compat {
        use std::{env, fs, io, path::PathBuf};
        pub struct TempDir(pub PathBuf);
        impl TempDir {
            pub fn new(prefix: &str) -> io::Result<Self> {
                let mut p = env::temp_dir();
                let n = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                p.push(format!("saw-spec-gen-{prefix}-{n}"));
                fs::create_dir_all(&p)?;
                Ok(TempDir(p))
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
}
