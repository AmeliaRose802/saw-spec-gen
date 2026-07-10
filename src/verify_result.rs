//! Shared `result.json` writers for native verification commands.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

pub const RESULT_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CounterexampleEntry {
    pub name: String,
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
}

#[derive(Serialize)]
struct VerifyResult<'a> {
    schema_version: &'static str,
    side: &'a str,
    function: &'a str,
    cryptol_fn: &'a str,
    verdict: &'a str,
    counterexample: &'a [CounterexampleEntry],
    expected: Option<&'a str>,
    actual: Option<&'a str>,
    solver: Option<&'a str>,
    time_secs: Option<f64>,
    impl_file: Option<&'a str>,
    reason_code: Option<&'a str>,
}

#[derive(Serialize)]
struct SpecOnlyResult<'a> {
    schema_version: &'static str,
    side: &'a str,
    function: &'a str,
    cryptol_fn: &'a str,
    verdict: &'a str,
    counterexample: &'a [CounterexampleEntry],
    expected: Option<&'a str>,
    actual: Option<&'a str>,
    solver: Option<&'a str>,
    time_secs: Option<f64>,
    impl_file: Option<&'a str>,
    status: &'a str,
    reason: &'a str,
    message: &'a str,
    kind: &'a str,
}

#[allow(clippy::too_many_arguments)]
pub fn write_verify_result(
    output_dir: &Path,
    side: &str,
    function: &str,
    cryptol_fn: &str,
    verdict: &str,
    counterexample: &[CounterexampleEntry],
    expected: Option<&str>,
    actual: Option<&str>,
    solver: Option<&str>,
    time_secs: Option<f64>,
    impl_file: Option<&str>,
    reason_code: Option<&str>,
) -> Result<()> {
    let payload = VerifyResult {
        schema_version: RESULT_SCHEMA_VERSION,
        side,
        function,
        cryptol_fn,
        verdict,
        counterexample,
        expected,
        actual,
        solver,
        time_secs,
        impl_file,
        reason_code,
    };
    write_payload(output_dir, &payload)
}

pub fn write_spec_only_result(
    output_dir: &Path,
    side: &str,
    function: &str,
    cryptol_fn: &str,
    reason: &str,
) -> Result<()> {
    let payload = SpecOnlyResult {
        schema_version: RESULT_SCHEMA_VERSION,
        side,
        function,
        cryptol_fn,
        verdict: "UNKNOWN",
        counterexample: &[],
        expected: None,
        actual: None,
        solver: None,
        time_secs: None,
        impl_file: None,
        status: "not_attempted",
        reason,
        message: reason,
        kind: "spec_only",
    };
    write_payload(output_dir, &payload)
}

fn write_payload<T: Serialize>(output_dir: &Path, payload: &T) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let mut text = serde_json::to_string_pretty(payload)?;
    text.push('\n');
    std::fs::write(output_dir.join("result.json"), text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_verified_shape() {
        let dir = tempdir_compat::TempDir::new("verify-result").unwrap();
        let cex = [CounterexampleEntry {
            name: "x".to_string(),
            value: Some("7".to_string()),
            bits: Some(32),
        }];
        write_verify_result(
            dir.path(),
            "cpp",
            "f",
            "f_spec",
            "DISPROVED",
            &cex,
            Some("8"),
            Some("0"),
            Some("z3"),
            Some(1.25),
            Some("f.cpp"),
            Some("disproved_counterexample"),
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(json["schema_version"], "1");
        assert_eq!(json["verdict"], "DISPROVED");
        assert_eq!(json["counterexample"][0]["bits"], 32);
        assert_eq!(json["expected"], "8");
        assert_eq!(json["actual"], "0");
        assert_eq!(json["solver"], "z3");
        assert_eq!(json["impl_file"], "f.cpp");
        assert_eq!(json["reason_code"], "disproved_counterexample");
    }

    #[test]
    fn writes_spec_only_compatibility_fields() {
        let dir = tempdir_compat::TempDir::new("verify-spec-only").unwrap();
        write_spec_only_result(dir.path(), "cpp", "f", "f_spec", "no implementation").unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(json["schema_version"], "1");
        assert_eq!(json["verdict"], "UNKNOWN");
        assert_eq!(json["status"], "not_attempted");
        assert_eq!(json["reason"], "no implementation");
    }

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
