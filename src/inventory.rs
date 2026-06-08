//! Inventory sidecar emission + aggregation for coverage matrix consumers.
//!
//! `gen-verify` / `gen-verify-rust` emit one `inventory.json` fragment per
//! verification run, and `aggregate-inventory` rolls those fragments up into a
//! single `implementation_inventory.json`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// One function entry in `implementation_inventory.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryEntry {
    pub name: String,
    pub lang: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composes: Vec<String>,
}

/// Top-level shape consumed by pretty-specs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImplementationInventory {
    pub functions: Vec<InventoryEntry>,
}

/// Emit one per-function `inventory.json` fragment into `output`.
pub fn emit_fragment(
    output: &Path,
    name: &str,
    lang: &str,
    symbol: Option<String>,
    file: Option<String>,
    cryptol_fn: &str,
    models_note: Option<String>,
) -> Result<()> {
    if name.trim().is_empty() {
        bail!("inventory entry requires a non-empty function name");
    }
    if lang != "cpp" && lang != "rust" {
        bail!("inventory entry lang must be 'cpp' or 'rust', got '{lang}'");
    }
    std::fs::create_dir_all(output)?;
    let fragment = InventoryEntry {
        name: name.to_string(),
        lang: lang.to_string(),
        symbol,
        file,
        models: (cryptol_fn != name).then(|| cryptol_fn.to_string()),
        models_note,
        composes: Vec::new(),
    };
    let mut text = serde_json::to_string_pretty(&fragment)?;
    text.push('\n');
    std::fs::write(output.join("inventory.json"), text)?;
    Ok(())
}

/// Build an optional models-note string from known bounded/ABI-adapter signals.
pub fn build_models_note(has_bound: bool, has_variant_return_map: bool) -> Option<String> {
    match (has_bound, has_variant_return_map) {
        (false, false) => None,
        (true, false) => Some("bounded model only".to_string()),
        (false, true) => Some("variant-mapped return via ABI adapter".to_string()),
        (true, true) => {
            Some("bounded model only; variant-mapped return via ABI adapter".to_string())
        }
    }
}

/// Aggregate all `inventory.json` fragments under `verify_out_dir` into
/// one `implementation_inventory.json` at `output`.
pub fn aggregate_inventory(verify_out_dir: &Path, output: &Path) -> Result<()> {
    if !verify_out_dir.is_dir() {
        bail!(
            "aggregate-inventory input must be a directory: {}",
            verify_out_dir.display()
        );
    }

    let mut files = Vec::new();
    collect_inventory_files(verify_out_dir, &mut files)?;
    files.sort();

    let mut functions = Vec::with_capacity(files.len());
    for path in &files {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading inventory fragment {}", path.display()))?;
        let entry: InventoryEntry = serde_json::from_str(&text)
            .with_context(|| format!("parsing inventory fragment {}", path.display()))?;
        functions.push(entry);
    }

    functions.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.lang.cmp(&b.lang))
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.file.cmp(&b.file))
    });

    let doc = ImplementationInventory { functions };
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut text = serde_json::to_string_pretty(&doc)?;
    text.push('\n');
    std::fs::write(output, text)?;
    Ok(())
}

fn collect_inventory_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_inventory_files(&path, out)?;
        } else if file_type.is_file() && path.file_name() == Some(OsStr::new("inventory.json")) {
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

/// Best-effort source-file lookup for a C++ function in a clang AST.
pub fn resolve_cpp_source_file(
    ast: &crate::clang_ast::AstNode,
    function: &str,
    target_mangled: &str,
) -> Option<String> {
    #[derive(Clone)]
    struct Candidate {
        score: u8,
        offset: u64,
        file: String,
    }

    struct Gather<'a> {
        function: &'a str,
        target_mangled: &'a str,
        current_file: Option<String>,
        candidates: Vec<Candidate>,
    }
    impl crate::clang_ast::Visitor for Gather<'_> {
        fn enter(&mut self, node: &crate::clang_ast::AstNode) -> crate::clang_ast::WalkAction {
            if let Some(loc_file) = node.loc.as_ref().and_then(|l| l.file.as_deref()) {
                self.current_file = Some(loc_file.to_string());
            }
            if !node.is_function_like() {
                return crate::clang_ast::WalkAction::Continue;
            }
            let name_matches = node.name.as_deref() == Some(self.function);
            let mangled_matches = node.mangled_name.as_deref() == Some(self.target_mangled);
            if !name_matches && !mangled_matches {
                return crate::clang_ast::WalkAction::Continue;
            }

            let has_body = node.inner.iter().any(|c| c.kind == "CompoundStmt");
            let file = node
                .loc
                .as_ref()
                .and_then(|l| l.file.clone())
                .or_else(|| self.current_file.clone());
            let Some(file) = file else {
                return crate::clang_ast::WalkAction::Continue;
            };
            let score = match (name_matches, mangled_matches, has_body) {
                (true, true, true) => 0,
                (_, true, true) => 1,
                (true, _, true) => 2,
                (true, true, false) => 3,
                (_, true, false) => 4,
                _ => 5,
            };
            self.candidates.push(Candidate {
                score,
                offset: node.source_offset(),
                file: file.replace('\\', "/"),
            });
            crate::clang_ast::WalkAction::Continue
        }
    }

    let mut gather = Gather {
        function,
        target_mangled,
        current_file: None,
        candidates: Vec::new(),
    };
    crate::clang_ast::walk(ast, &mut gather);
    gather
        .candidates
        .into_iter()
        .min_by(|a, b| a.score.cmp(&b.score).then_with(|| a.offset.cmp(&b.offset)))
        .map(|c| c.file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempdir_compat::TempDir;

    #[test]
    fn emits_fragment_with_expected_optional_fields() {
        let dir = TempDir::new("inventory-fragment").unwrap();
        emit_fragment(
            dir.path(),
            "provisionKey",
            "cpp",
            Some("?provisionKey@@YAXH@Z".to_string()),
            Some("cpp/src/decision.cpp".to_string()),
            "provisionKey_model",
            Some("bounded model only".to_string()),
        )
        .unwrap();

        let text = std::fs::read_to_string(dir.path().join("inventory.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["name"], "provisionKey");
        assert_eq!(v["lang"], "cpp");
        assert_eq!(v["symbol"], "?provisionKey@@YAXH@Z");
        assert_eq!(v["file"], "cpp/src/decision.cpp");
        assert_eq!(v["models"], "provisionKey_model");
        assert_eq!(v["models_note"], "bounded model only");
        assert!(
            v.get("composes").is_none(),
            "composes should be omitted when empty"
        );
    }

    #[test]
    fn aggregates_fragments_into_single_document() {
        let dir = TempDir::new("inventory-aggregate").unwrap();
        let out_a = dir.path().join("out_a");
        let out_b = dir.path().join("out_b");
        emit_fragment(
            &out_a,
            "alpha",
            "cpp",
            Some("_Z5alphav".to_string()),
            Some("cpp/alpha.cpp".to_string()),
            "alpha",
            None,
        )
        .unwrap();
        emit_fragment(
            &out_b,
            "beta",
            "rust",
            Some("_RNvCsbeta4beta".to_string()),
            None,
            "beta_spec",
            Some("variant-mapped return via ABI adapter".to_string()),
        )
        .unwrap();

        let output = dir.path().join("implementation_inventory.json");
        aggregate_inventory(dir.path(), &output).unwrap();

        let text = std::fs::read_to_string(output).unwrap();
        let doc: ImplementationInventory = serde_json::from_str(&text).unwrap();
        assert_eq!(doc.functions.len(), 2);
        assert_eq!(doc.functions[0].name, "alpha");
        assert_eq!(doc.functions[1].name, "beta");
        assert_eq!(doc.functions[1].models.as_deref(), Some("beta_spec"));
    }

    // Minimal in-tree temp directory shim — avoids extra dev dependencies.
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
