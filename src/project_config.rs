//! Project-level configuration loaded from `saw-spec-gen.toml`.
//!
//! These fields are the *only* way to shape generated specs. All spec
//! shaping (buffer sizes, out-buffer bindings, variant maps, alias
//! overrides, recognizer toggles) is configured here; the corresponding
//! `gen-verify` CLI flags have been removed. Per-function tables win over
//! the global values — see [`ProjectConfig::apply`].
//!
//! ## Auto-discovery
//!
//! `ProjectConfig::discover(dir)` walks from `dir` up to the filesystem root
//! looking for the first `saw-spec-gen.toml` it finds — the same way rustfmt
//! and cargo locate their config files.  Pass `--config PATH` to point at an
//! explicit file instead.
//!
//! ## Example `saw-spec-gen.toml`
//!
//! ```toml
//! # Global alias-size overrides applied to every gen-verify call
//! alias_size = ["MyOpaque=16"]
//!
//! # Per-function shaping, keyed by Cryptol fn name. Applies only when
//! # gen-verify runs with `--cryptol-fn canonicalize_lp`. Resolved before
//! # (and overriding) the global values.
//! [functions.canonicalize_lp]
//! in_buffer_size   = ["m=4", "b=4"]
//! out_buffer_param = ["out=10"]
//! cryptol_fn_out   = ["out=canonicalize_lp_post"]
//! max_len_precond  = ["nm=4", "nb=4"]
//! ```

use crate::uninterpreted::{ComposeEntry, UninterpretedEntry};
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

    /// Per-function extra `llvm_precond` clauses. Each string is a raw
    /// Cryptol predicate emitted verbatim as `llvm_precond {{ <expr> }}`
    /// after the fresh symbolic inputs are bound and before
    /// `llvm_execute_func`. Use it to constrain object/buffer bytes the
    /// generator models as an opaque byte array — e.g. pinning a
    /// `bool`/`std::optional` engaged flag to a canonical value
    /// (`(this_pre @ 128) <= 1`) so the C++ `trunc i8 to i1` read agrees
    /// with a Cryptol model that tests `== 1`.
    #[serde(default)]
    pub preconditions: Vec<String>,

    /// Per-function `sret_assert_bytes = N`: assert only the first `N`
    /// bytes of the sret aggregate return, leaving trailing undefined
    /// bytes (e.g. `std::optional` padding) unconstrained.
    #[serde(default)]
    pub sret_assert_bytes: Option<usize>,

    /// `[[functions.<caller>.compose]]` blocks: cross-TU callees to
    /// override with their proven Cryptol contracts (assume-guarantee)
    /// instead of the default fresh-return/havoc extern model. See
    /// [`crate::uninterpreted::ComposeEntry`] and
    /// `docs/25-compositional-contract-overrides.md`.
    #[serde(default)]
    pub compose: Vec<ComposeEntry>,

    /// Per-function `combine_scope = "callgraph"|"explicit"`: how to make
    /// composed callee symbols available. `callgraph` (default) relies on
    /// the caller module already carrying the callee as a `declare`, which
    /// is sufficient for `llvm_unsafe_assume_spec`. Reserved for future
    /// callgraph-scoped module linking.
    #[serde(default)]
    pub combine_scope: Option<String>,

    /// Single-contract model (see docs/27): when set, the return value is
    /// sourced from a *field* of the record returned by the Cryptol
    /// contract function (whose name is the `--cryptol-fn`), i.e.
    /// `llvm_return ((<cryptol_fn> args).<contract_return>)`. Lets one
    /// record-returning contract carry every effect of a function.
    #[serde(default)]
    pub contract_return: Option<String>,

    /// Single-contract post-state bindings: each `REGION=FIELD` binds the
    /// out-buffer `REGION`'s post-state to `(<cryptol_fn> args).FIELD`.
    /// `REGION` must also be declared via `out_buffer_param`. See docs/27.
    #[serde(default)]
    pub contract_ensures: Vec<String>,
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

    /// Global extra `llvm_precond` clauses (raw Cryptol predicates).
    /// See [`FunctionConfig::preconditions`].
    #[serde(default)]
    pub preconditions: Vec<String>,

    /// Global `sret_assert_bytes = N`. See
    /// [`FunctionConfig::sret_assert_bytes`].
    #[serde(default)]
    pub sret_assert_bytes: Option<usize>,

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

    /// Global single-contract return-field binding. See
    /// [`FunctionConfig::contract_return`].
    #[serde(default)]
    pub contract_return: Option<String>,

    /// Global single-contract post-state bindings. See
    /// [`FunctionConfig::contract_ensures`].
    #[serde(default)]
    pub contract_ensures: Vec<String>,
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

    /// Merge per-function config with global config for the Cryptol
    /// function named `function`.
    ///
    /// For `Vec` fields both layers are concatenated (per-function
    /// entries first, then global), preserving the ordering the previous
    /// CLI-aware merge used. For booleans, `true` from either layer wins.
    /// There is no CLI layer — shaping is config-only.
    pub fn apply(&self, function: &str) -> MergedConfig {
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
        let pf_preconditions = f.map_or(&[][..], |c| &c.preconditions);

        let pf_bool =
            |sel: fn(&FunctionConfig) -> Option<bool>| -> bool { f.and_then(sel).unwrap_or(false) };

        MergedConfig {
            no_struct_shape_recognizer: pf_bool(|c| c.no_struct_shape_recognizer)
                || self.no_struct_shape_recognizer.unwrap_or(false),
            use_llvm_combine_modules: pf_bool(|c| c.use_llvm_combine_modules)
                || self.use_llvm_combine_modules.unwrap_or(false),
            spec_only_on_missing: pf_bool(|c| c.spec_only_on_missing)
                || self.spec_only_on_missing.unwrap_or(false),
            alias_size: merged_vec(pf_alias_size, &self.alias_size),
            alias_enum: merged_vec(pf_alias_enum, &self.alias_enum),
            in_buffer_size: merged_vec(pf_in_buffer, &self.in_buffer_size),
            max_len_precond: merged_vec(pf_max_len, &self.max_len_precond),
            out_buffer_param: merged_vec(pf_out_buffer, &self.out_buffer_param),
            cryptol_fn_out: merged_vec(pf_fn_out, &self.cryptol_fn_out),
            cryptol_fn_pre: merged_vec(pf_fn_pre, &self.cryptol_fn_pre),
            cryptol_arg_order: merged_vec(pf_arg_order, &self.cryptol_arg_order),
            variant_map: merged_vec(pf_variant_map, &self.variant_map),
            preconditions: merged_vec(pf_preconditions, &self.preconditions),
            sret_assert_bytes: f
                .and_then(|c| c.sret_assert_bytes)
                .or(self.sret_assert_bytes),
            uninterpreted: self.uninterpreted.clone(),
            compose: f.map(|c| c.compose.clone()).unwrap_or_default(),
            combine_scope: f.and_then(|c| c.combine_scope.clone()),
            contract_return: f
                .and_then(|c| c.contract_return.clone())
                .or_else(|| self.contract_return.clone()),
            contract_ensures: merged_vec(
                f.map_or(&[][..], |c| &c.contract_ensures),
                &self.contract_ensures,
            ),
        }
    }
}

/// Concatenate the per-function and global `Vec<String>` layers in the
/// same order the previous CLI-aware merge used (per-function first, then
/// global). Later layers extend, never replace.
fn merged_vec(per_fn: &[String], global: &[String]) -> Vec<String> {
    let mut v = Vec::with_capacity(per_fn.len() + global.len());
    v.extend(per_fn.iter().cloned());
    v.extend(global.iter().cloned());
    v
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
    /// Extra raw `llvm_precond` clauses (config-only; no CLI equivalent).
    pub preconditions: Vec<String>,
    /// Assert only the first N bytes of an sret return (config-only).
    pub sret_assert_bytes: Option<usize>,
    /// `[[uninterpreted]]` entries (config-only; no CLI equivalent).
    pub uninterpreted: Vec<UninterpretedEntry>,
    /// Per-function `compose` entries: cross-TU callee contracts
    /// (assume-guarantee). Config-only; no CLI equivalent.
    pub compose: Vec<ComposeEntry>,
    /// Per-function `combine_scope`. Config-only; no CLI equivalent.
    pub combine_scope: Option<String>,
    /// Single-contract return-field binding (docs/27). Config-only.
    pub contract_return: Option<String>,
    /// Single-contract post-state bindings `REGION=FIELD` (docs/27).
    pub contract_ensures: Vec<String>,
}

impl MergedConfig {
    /// Full `--cryptol-fn-out` list, appending entries desugared from
    /// `contract_ensures`: each `REGION=FIELD` becomes
    /// `REGION=<cryptol_fn>.FIELD`, so one record-returning contract
    /// supplies every out-region post-state via field projection.
    pub fn cryptol_fn_out_with_contract(&self, cryptol_fn: &str) -> Result<Vec<String>> {
        let mut v = self.cryptol_fn_out.clone();
        for e in &self.contract_ensures {
            let (region, field) = e.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("contract_ensures entry must be REGION=FIELD, got {e:?}")
            })?;
            v.push(format!("{}={}.{}", region.trim(), cryptol_fn, field.trim()));
        }
        Ok(v)
    }
}

#[cfg(test)]
#[path = "project_config_tests.rs"]
mod tests;
