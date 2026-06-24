//! Project-level configuration loaded from `saw-spec-gen.toml`.
//!
//! All fields mirror CLI flags on the `gen-verify` subcommand and act as
//! project-wide defaults.  **CLI flags always win**: a flag explicitly
//! passed on the command line overrides the config value.
//!
//! ## Auto-discovery
//!
//! `ProjectConfig::discover(dir)` walks from `dir` up to the filesystem root
//! looking for the first `saw-spec-gen.toml` it finds — the same way rustfmt
//! and cargo locate their config files.  No `--config` flag is needed when
//! the file lives at the repo root.
//!
//! ## Example `saw-spec-gen.toml`
//!
//! ```toml
//! # Global alias-size overrides applied to every gen-verify call
//! alias_size = ["MyOpaque=16"]
//! ```

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Deserialised contents of a `saw-spec-gen.toml` file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Equivalent to `--no-struct-shape-recognizer`.
    pub no_struct_shape_recognizer: Option<bool>,

    /// Equivalent to `--use-llvm-combine-modules`.
    pub use_llvm_combine_modules: Option<bool>,

    /// Equivalent to `--spec-only-on-missing`.
    pub spec_only_on_missing: Option<bool>,

    /// Equivalent to repeated `--alias-size NAME=BYTES`.
    #[serde(default)]
    pub alias_size: Vec<String>,

    /// Equivalent to repeated `--alias-enum NAME=BITS`.
    #[serde(default)]
    pub alias_enum: Vec<String>,

    /// Equivalent to repeated `--in-buffer-size NAME=BYTES`.
    #[serde(default)]
    pub in_buffer_size: Vec<String>,

    /// Equivalent to repeated `--max-len-precond NAME=VAL`.
    #[serde(default)]
    pub max_len_precond: Vec<String>,
}

impl ProjectConfig {
    /// Load config from an explicit path.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
    }

    /// Return the path of a config file that is a sibling of `spec_path`,
    /// sharing its stem with a `.toml` extension (e.g. `count_bytes_spec.toml`
    /// next to `count_bytes_spec.cry`), or `None` if no such file exists.
    pub fn sibling_path(spec_path: &Path) -> Option<PathBuf> {
        let candidate = spec_path.with_extension("toml");
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    }

    /// Walk from `start_dir` toward the filesystem root, returning the path
    /// of the first `saw-spec-gen.toml` found, or `None`.
    pub fn find(start_dir: &Path) -> Option<PathBuf> {
        let mut dir = start_dir.to_path_buf();
        loop {
            let candidate = dir.join("saw-spec-gen.toml");
            if candidate.exists() {
                return Some(candidate);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    /// Locate and load the config for a given Cryptol spec file.
    ///
    /// Search order (first match wins):
    /// 1. `<spec_stem>.toml` — sibling of the spec file (per-spec config)
    /// 2. `saw-spec-gen.toml` walking up from the spec's directory
    /// 3. `saw-spec-gen.toml` walking up from `fallback_dir` (typically cwd)
    pub fn discover_for_spec(spec_path: &Path, fallback_dir: &Path) -> Result<Self> {
        if let Some(p) = Self::sibling_path(spec_path) {
            eprintln!("Using project config: {}", p.display());
            return Self::load(&p);
        }
        let spec_dir = spec_path.parent().unwrap_or(fallback_dir);
        if let Some(p) = Self::find(spec_dir) {
            eprintln!("Using project config: {}", p.display());
            return Self::load(&p);
        }
        match Self::find(fallback_dir) {
            Some(p) => {
                eprintln!("Using project config: {}", p.display());
                Self::load(&p)
            }
            None => Ok(Self::default()),
        }
    }

    /// Merge CLI values over this config.  For booleans, `true` from either
    /// source wins.  For `Vec` fields, config entries come first so that CLI
    /// entries can extend (not replace) them.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        cli_no_struct_shape_recognizer: bool,
        cli_use_llvm_combine_modules: bool,
        cli_spec_only_on_missing: bool,
        cli_alias_size: Vec<String>,
        cli_alias_enum: Vec<String>,
        cli_in_buffer_size: Vec<String>,
        cli_max_len_precond: Vec<String>,
    ) -> MergedConfig {
        let mut alias_size = self.alias_size.clone();
        alias_size.extend(cli_alias_size);

        let mut alias_enum = self.alias_enum.clone();
        alias_enum.extend(cli_alias_enum);

        let mut in_buffer_size = self.in_buffer_size.clone();
        in_buffer_size.extend(cli_in_buffer_size);

        let mut max_len_precond = self.max_len_precond.clone();
        max_len_precond.extend(cli_max_len_precond);

        MergedConfig {
            no_struct_shape_recognizer: cli_no_struct_shape_recognizer
                || self.no_struct_shape_recognizer.unwrap_or(false),
            use_llvm_combine_modules: cli_use_llvm_combine_modules
                || self.use_llvm_combine_modules.unwrap_or(false),
            spec_only_on_missing: cli_spec_only_on_missing
                || self.spec_only_on_missing.unwrap_or(false),
            alias_size,
            alias_enum,
            in_buffer_size,
            max_len_precond,
        }
    }
}

/// Fully-resolved values after merging the project config with CLI flags.
pub struct MergedConfig {
    pub no_struct_shape_recognizer: bool,
    pub use_llvm_combine_modules: bool,
    pub spec_only_on_missing: bool,
    pub alias_size: Vec<String>,
    pub alias_enum: Vec<String>,
    pub in_buffer_size: Vec<String>,
    pub max_len_precond: Vec<String>,
}
