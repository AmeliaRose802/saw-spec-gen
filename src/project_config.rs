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
//!
//! # Per-function shaping, keyed by Cryptol fn name. Applies only when
//! # gen-verify runs with `--cryptol-fn canonicalize_lp`. Resolved before
//! # the global values and any CLI flags.
//! [functions.canonicalize_lp]
//! in_buffer_size   = ["m=4", "b=4"]
//! out_buffer_param = ["out=10"]
//! cryptol_fn_out   = ["out=canonicalize_lp_post"]
//! max_len_precond  = ["nm=4", "nb=4"]
//! ```

use crate::uninterpreted::UninterpretedEntry;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Per-function spec-shaping overrides, keyed by Cryptol function name
/// under a `[functions.<cryptol_fn>]` table.
///
/// Every field mirrors a `gen-verify` CLI flag that is inherently
/// per-function (buffer shapes, out-buffer Cryptol bindings, argument
/// ordering, variant maps). Values declared here apply only when the
/// `--cryptol-fn` being generated matches the table key. They are
/// resolved *before* the global config and CLI flags — see
/// [`ProjectConfig::apply`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionConfig {
    /// Per-function `--no-struct-shape-recognizer`.
    pub no_struct_shape_recognizer: Option<bool>,

    /// Per-function `--use-llvm-combine-modules`.
    pub use_llvm_combine_modules: Option<bool>,

    /// Per-function `--spec-only-on-missing`.
    pub spec_only_on_missing: Option<bool>,

    /// Per-function `--alias-size NAME=BYTES`.
    #[serde(default)]
    pub alias_size: Vec<String>,

    /// Per-function `--alias-enum NAME=BITS`.
    #[serde(default)]
    pub alias_enum: Vec<String>,

    /// Per-function `--in-buffer-size NAME=SHAPE`.
    #[serde(default)]
    pub in_buffer_size: Vec<String>,

    /// Per-function `--max-len-precond NAME=VAL`.
    #[serde(default)]
    pub max_len_precond: Vec<String>,

    /// Per-function `--out-buffer-param NAME=SHAPE|auto`.
    #[serde(default)]
    pub out_buffer_param: Vec<String>,

    /// Per-function `--cryptol-fn-out OUT_PARAM=FN`.
    #[serde(default)]
    pub cryptol_fn_out: Vec<String>,

    /// Per-function `--cryptol-fn-pre FN`.
    #[serde(default)]
    pub cryptol_fn_pre: Vec<String>,

    /// Per-function `--cryptol-arg-order FN=arg1,arg2,...`.
    #[serde(default)]
    pub cryptol_arg_order: Vec<String>,

    /// Per-function `--variant-map PARAM=V1:D1,V2:D2,...`.
    #[serde(default)]
    pub variant_map: Vec<String>,
}

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

    /// Equivalent to repeated `--out-buffer-param NAME=SHAPE|auto`.
    #[serde(default)]
    pub out_buffer_param: Vec<String>,

    /// Equivalent to repeated `--cryptol-fn-out OUT_PARAM=FN`.
    #[serde(default)]
    pub cryptol_fn_out: Vec<String>,

    /// Equivalent to `--cryptol-fn-pre FN`.
    #[serde(default)]
    pub cryptol_fn_pre: Vec<String>,

    /// Equivalent to repeated `--cryptol-arg-order FN=arg1,...`.
    #[serde(default)]
    pub cryptol_arg_order: Vec<String>,

    /// Equivalent to repeated `--variant-map PARAM=V1:D1,...`.
    #[serde(default)]
    pub variant_map: Vec<String>,

    /// `[functions.<cryptol_fn>]` tables: per-function spec-shaping
    /// overrides. Keyed by the `--cryptol-fn` name. See
    /// [`FunctionConfig`].
    #[serde(default)]
    pub functions: HashMap<String, FunctionConfig>,

    /// `[[uninterpreted]]` blocks: opaque callees (crypto primitives,
    /// etc.) bound to a Cryptol contract via `llvm_unsafe_assume_spec`
    /// instead of being symbolically executed. See
    /// [`crate::uninterpreted`]. No CLI flag — config + Cryptol
    /// `@uninterpreted` annotations are the only declaration surfaces.
    #[serde(default)]
    pub uninterpreted: Vec<UninterpretedEntry>,
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

    /// Merge per-function config, global config, and CLI flags for the
    /// Cryptol function named `function`.
    ///
    /// Resolution order, low precedence first: **per-function config →
    /// global config → CLI**. For `Vec` fields all three layers are
    /// concatenated in that order (CLI entries extend, never replace, the
    /// config-declared ones). For booleans, `true` from any layer wins.
    pub fn apply(&self, function: &str, cli: CliFlags) -> MergedConfig {
        let f = self.functions.get(function);

        // Per-function slices (empty when no `[functions.<fn>]` table).
        let pf_alias_size = f.map_or(&[][..], |c| &c.alias_size);
        let pf_alias_enum = f.map_or(&[][..], |c| &c.alias_enum);
        let pf_in_buffer = f.map_or(&[][..], |c| &c.in_buffer_size);
        let pf_max_len = f.map_or(&[][..], |c| &c.max_len_precond);
        let pf_out_buffer = f.map_or(&[][..], |c| &c.out_buffer_param);
        let pf_fn_out = f.map_or(&[][..], |c| &c.cryptol_fn_out);
        let pf_fn_pre = f.map_or(&[][..], |c| &c.cryptol_fn_pre);
        let pf_arg_order = f.map_or(&[][..], |c| &c.cryptol_arg_order);
        let pf_variant_map = f.map_or(&[][..], |c| &c.variant_map);

        let pf_bool =
            |sel: fn(&FunctionConfig) -> Option<bool>| -> bool { f.and_then(sel).unwrap_or(false) };

        MergedConfig {
            no_struct_shape_recognizer: cli.no_struct_shape_recognizer
                || pf_bool(|c| c.no_struct_shape_recognizer)
                || self.no_struct_shape_recognizer.unwrap_or(false),
            use_llvm_combine_modules: cli.use_llvm_combine_modules
                || pf_bool(|c| c.use_llvm_combine_modules)
                || self.use_llvm_combine_modules.unwrap_or(false),
            spec_only_on_missing: cli.spec_only_on_missing
                || pf_bool(|c| c.spec_only_on_missing)
                || self.spec_only_on_missing.unwrap_or(false),
            alias_size: merged_vec(pf_alias_size, &self.alias_size, cli.alias_size),
            alias_enum: merged_vec(pf_alias_enum, &self.alias_enum, cli.alias_enum),
            in_buffer_size: merged_vec(pf_in_buffer, &self.in_buffer_size, cli.in_buffer_size),
            max_len_precond: merged_vec(pf_max_len, &self.max_len_precond, cli.max_len_precond),
            out_buffer_param: merged_vec(
                pf_out_buffer,
                &self.out_buffer_param,
                cli.out_buffer_param,
            ),
            cryptol_fn_out: merged_vec(pf_fn_out, &self.cryptol_fn_out, cli.cryptol_fn_out),
            cryptol_fn_pre: merged_vec(pf_fn_pre, &self.cryptol_fn_pre, cli.cryptol_fn_pre),
            cryptol_arg_order: merged_vec(
                pf_arg_order,
                &self.cryptol_arg_order,
                cli.cryptol_arg_order,
            ),
            variant_map: merged_vec(pf_variant_map, &self.variant_map, cli.variant_map),
            uninterpreted: self.uninterpreted.clone(),
        }
    }
}

/// Concatenate per-function, global, and CLI `Vec<String>` layers in
/// precedence order (low → high). Later layers extend, never replace.
fn merged_vec(per_fn: &[String], global: &[String], cli: Vec<String>) -> Vec<String> {
    let mut v = Vec::with_capacity(per_fn.len() + global.len() + cli.len());
    v.extend(per_fn.iter().cloned());
    v.extend(global.iter().cloned());
    v.extend(cli);
    v
}

/// CLI flag values forwarded into [`ProjectConfig::apply`]. Bundled into a
/// struct so the merge signature stays readable as the set of shaping
/// flags grows.
#[derive(Debug, Default)]
pub struct CliFlags {
    pub no_struct_shape_recognizer: bool,
    pub use_llvm_combine_modules: bool,
    pub spec_only_on_missing: bool,
    pub alias_size: Vec<String>,
    pub alias_enum: Vec<String>,
    pub in_buffer_size: Vec<String>,
    pub max_len_precond: Vec<String>,
    pub out_buffer_param: Vec<String>,
    pub cryptol_fn_out: Vec<String>,
    pub cryptol_fn_pre: Vec<String>,
    pub cryptol_arg_order: Vec<String>,
    pub variant_map: Vec<String>,
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
    pub out_buffer_param: Vec<String>,
    pub cryptol_fn_out: Vec<String>,
    pub cryptol_fn_pre: Vec<String>,
    pub cryptol_arg_order: Vec<String>,
    pub variant_map: Vec<String>,
    /// `[[uninterpreted]]` entries (config-only; no CLI equivalent).
    pub uninterpreted: Vec<UninterpretedEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn per_function_table_parses_from_toml() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
            in_buffer_size = ["g=1"]

            [functions.canonicalize_lp]
            in_buffer_size   = ["m=4", "b=4"]
            out_buffer_param = ["out=10"]
            cryptol_fn_out   = ["out=canonicalize_lp_post"]
            max_len_precond  = ["nm=4", "nb=4"]
            "#,
        )
        .expect("config parses");

        let f = cfg.functions.get("canonicalize_lp").expect("table present");
        assert_eq!(f.in_buffer_size, v(&["m=4", "b=4"]));
        assert_eq!(f.out_buffer_param, v(&["out=10"]));
        assert_eq!(f.cryptol_fn_out, v(&["out=canonicalize_lp_post"]));
        assert_eq!(f.max_len_precond, v(&["nm=4", "nb=4"]));
    }

    #[test]
    fn apply_concatenates_per_function_global_then_cli() {
        let mut cfg = ProjectConfig {
            in_buffer_size: v(&["global=2"]),
            ..Default::default()
        };
        cfg.functions.insert(
            "canonicalize_lp".to_string(),
            FunctionConfig {
                in_buffer_size: v(&["m=4", "b=4"]),
                out_buffer_param: v(&["out=10"]),
                cryptol_fn_out: v(&["out=canonicalize_lp_post"]),
                ..Default::default()
            },
        );

        let merged = cfg.apply(
            "canonicalize_lp",
            CliFlags {
                in_buffer_size: v(&["cli=8"]),
                ..Default::default()
            },
        );

        // per-function, then global, then CLI.
        assert_eq!(
            merged.in_buffer_size,
            v(&["m=4", "b=4", "global=2", "cli=8"])
        );
        assert_eq!(merged.out_buffer_param, v(&["out=10"]));
        assert_eq!(merged.cryptol_fn_out, v(&["out=canonicalize_lp_post"]));
    }

    #[test]
    fn apply_falls_back_to_global_when_function_absent() {
        let cfg = ProjectConfig {
            out_buffer_param: v(&["g=1"]),
            ..Default::default()
        };
        let merged = cfg.apply(
            "no_such_fn",
            CliFlags {
                out_buffer_param: v(&["c=2"]),
                ..Default::default()
            },
        );
        assert_eq!(merged.out_buffer_param, v(&["g=1", "c=2"]));
    }

    #[test]
    fn boolean_true_from_any_layer_wins() {
        let mut cfg = ProjectConfig::default();
        cfg.functions.insert(
            "f".to_string(),
            FunctionConfig {
                spec_only_on_missing: Some(true),
                ..Default::default()
            },
        );
        // per-function true, no global, no CLI.
        let merged = cfg.apply("f", CliFlags::default());
        assert!(merged.spec_only_on_missing);

        // global true reaches an unrelated function.
        let cfg2 = ProjectConfig {
            use_llvm_combine_modules: Some(true),
            ..Default::default()
        };
        let merged2 = cfg2.apply("other", CliFlags::default());
        assert!(merged2.use_llvm_combine_modules);
    }
}
