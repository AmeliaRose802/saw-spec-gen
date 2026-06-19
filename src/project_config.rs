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
//! # Project-wide gen-verify defaults
//! bind_cryptol_lengths = true
//!
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
    /// Equivalent to `--bind-cryptol-lengths`.
    pub bind_cryptol_lengths: Option<bool>,

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
        toml::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))
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

    /// Load the first `saw-spec-gen.toml` found walking up from `start_dir`,
    /// or return a default (all-`None`) config if none is found.
    pub fn discover(start_dir: &Path) -> Result<Self> {
        match Self::find(start_dir) {
            Some(p) => {
                eprintln!("Using project config: {}", p.display());
                Self::load(&p)
            }
            None => Ok(Self::default()),
        }
    }

    /// Like [`discover`], but also searches from `hint_dir` before `start_dir`.
    ///
    /// Use this when the files being verified live in a subdirectory (e.g.
    /// a test-case directory).  The config closest to the source files wins;
    /// if none is found there the search continues from `start_dir` (typically
    /// `cwd`).
    pub fn discover_with_hint(hint_dir: &Path, fallback_dir: &Path) -> Result<Self> {
        // Prefer a config that lives near the source/spec files.
        if let Some(p) = Self::find(hint_dir) {
            eprintln!("Using project config: {}", p.display());
            return Self::load(&p);
        }
        Self::discover(fallback_dir)
    }

    /// Merge CLI values over this config.  For booleans, `true` from either
    /// source wins.  For `Vec` fields, config entries come first so that CLI
    /// entries can extend (not replace) them.
    pub fn apply(
        &self,
        cli_bind_cryptol_lengths: bool,
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
            bind_cryptol_lengths: cli_bind_cryptol_lengths
                || self.bind_cryptol_lengths.unwrap_or(false),
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
    pub bind_cryptol_lengths: bool,
    pub no_struct_shape_recognizer: bool,
    pub use_llvm_combine_modules: bool,
    pub spec_only_on_missing: bool,
    pub alias_size: Vec<String>,
    pub alias_enum: Vec<String>,
    pub in_buffer_size: Vec<String>,
    pub max_len_precond: Vec<String>,
}
