use anyhow::Result;
use serde::Serialize;
use std::path::Path;

pub const RESULT_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResultCounterexample {
    pub name: String,
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyResultJson {
    pub schema_version: &'static str,
    pub side: String,
    pub function: String,
    pub cryptol_fn: String,
    pub verdict: String,
    pub counterexample: Vec<ResultCounterexample>,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub solver: Option<String>,
    pub time_secs: Option<f64>,
    pub impl_file: Option<String>,
}

impl VerifyResultJson {
    pub fn new(side: &str, function: &str, cryptol_fn: &str, verdict: &str) -> Self {
        Self {
            schema_version: RESULT_SCHEMA_VERSION,
            side: side.to_string(),
            function: function.to_string(),
            cryptol_fn: cryptol_fn.to_string(),
            verdict: verdict.to_string(),
            counterexample: vec![],
            expected: None,
            actual: None,
            solver: None,
            time_secs: None,
            impl_file: None,
        }
    }
}

pub fn write_verify_result(output_dir: &Path, payload: &VerifyResultJson) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let result_path = output_dir.join("result.json");
    std::fs::write(result_path, serde_json::to_string_pretty(payload)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_schema_1_shape() {
        let mut payload = VerifyResultJson::new("rust", "add_one", "add_one_spec", "DISPROVED");
        payload.counterexample.push(ResultCounterexample {
            name: "x".to_string(),
            value: Some("7".to_string()),
            bits: Some(32),
        });
        let v = serde_json::to_value(payload).expect("serialize");
        assert_eq!(v["schema_version"], "1");
        assert_eq!(v["side"], "rust");
        assert_eq!(v["counterexample"][0]["name"], "x");
        assert_eq!(v["counterexample"][0]["bits"], 32);
        assert!(v.get("expected").is_some());
        assert!(v.get("actual").is_some());
        assert!(v.get("solver").is_some());
        assert!(v.get("time_secs").is_some());
        assert!(v.get("impl_file").is_some());
    }
}
