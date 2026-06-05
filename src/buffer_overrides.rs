//! Per-target overrides for buffer-shape parameters and Cryptol arg ordering.
//!
//! Exposed via the `gen-verify` CLI flags:
//!   --in-buffer-size   NAME=BYTES
//!   --out-buffer-param NAME=BYTES
//!   --cryptol-fn-out   OUT_PARAM=FN
//!   --max-len-precond  NAME=VAL
//!   --cryptol-arg-order FN=arg1,arg2,...
//!
//! Motivation: targets whose C++ signature carries raw `uint8_t*`
//! pointers + a separate scalar return value need to be verified
//! against two Cryptol functions — one modelling the output buffer
//! post-state, one modelling the return value — and need explicit
//! buffer sizes (the AST has no `_Out_writes_(N)` annotation on the
//! production headers). Hand-written SAW specs used to fill the gap;
//! these flags let `gen-verify` emit the same spec automatically.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;

/// One position in an explicit Cryptol-side argument list, derived
/// from a `--cryptol-arg-order FN=arg1,arg2,...` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryArg {
    /// Plain C++ parameter — substitute the SAW value variable bound
    /// to that param (which is `<param.name>` for read-only / scalar
    /// params and `<param.name>_pre` for params declared as
    /// `--out-buffer-param`).
    Param(String),
    /// Pre-call state of a C++ buffer parameter — substitute
    /// `<param>_pre`. Use this in `--cryptol-arg-order` to be
    /// explicit even when the param is also a regular `--in-buffer-size`.
    PreParam(String),
}

#[derive(Debug, Default, Clone)]
pub struct BufferOverrides {
    /// `--in-buffer-size NAME=BYTES` — pointer param NAME is a
    /// read-only input buffer of BYTES bytes (emit
    /// `llvm_alloc_readonly (llvm_array BYTES (llvm_int 8))`).
    pub in_buffer_sizes: HashMap<String, usize>,

    /// `--out-buffer-param NAME=BYTES` — pointer param NAME is a
    /// writable output buffer of BYTES bytes. The emitter:
    ///   - allocates `NAME_ptr` with `llvm_alloc (llvm_array BYTES (llvm_int 8))`
    ///   - binds a fresh `NAME_pre` to its pre-call contents
    ///   - post-asserts the new contents via `--cryptol-fn-out NAME=FN`
    pub out_buffer_sizes: HashMap<String, usize>,

    /// `--cryptol-fn-out NAME=FN` — Cryptol function modelling the
    /// post-call contents of out-buffer NAME.
    pub cryptol_fn_out: HashMap<String, String>,

    /// `--max-len-precond NAME=VAL` — emit `llvm_precond {{ NAME <= VAL }}`.
    /// Order preserved so the generated script is deterministic.
    pub max_len_preconds: Vec<(String, u64)>,

    /// `--cryptol-arg-order FN=arg1,arg2,...` — explicit Cryptol arg
    /// order when the model takes a subset / reorder / pre-state of
    /// the C++ parameter list.
    pub cryptol_arg_orders: HashMap<String, Vec<CryArg>>,
}

impl BufferOverrides {
    pub fn from_cli(
        in_buffer_size: &[String],
        out_buffer_param: &[String],
        cryptol_fn_out: &[String],
        max_len_precond: &[String],
        cryptol_arg_order: &[String],
    ) -> Result<Self> {
        let mut me = BufferOverrides::default();
        for s in in_buffer_size {
            let (k, v) = split_name_eq(s, "--in-buffer-size")?;
            let bytes: usize = v.parse().with_context(|| {
                format!("--in-buffer-size {s}: expected NAME=<unsigned integer>")
            })?;
            me.in_buffer_sizes.insert(k.to_string(), bytes);
        }
        for s in out_buffer_param {
            let (k, v) = split_name_eq(s, "--out-buffer-param")?;
            let bytes: usize = v.parse().with_context(|| {
                format!("--out-buffer-param {s}: expected NAME=<unsigned integer>")
            })?;
            me.out_buffer_sizes.insert(k.to_string(), bytes);
        }
        for s in cryptol_fn_out {
            let (k, v) = split_name_eq(s, "--cryptol-fn-out")?;
            me.cryptol_fn_out.insert(k.to_string(), v.to_string());
        }
        for s in max_len_precond {
            let (k, v) = split_name_eq(s, "--max-len-precond")?;
            let val: u64 = v.parse().with_context(|| {
                format!("--max-len-precond {s}: expected NAME=<unsigned integer>")
            })?;
            me.max_len_preconds.push((k.to_string(), val));
        }
        for s in cryptol_arg_order {
            let (fn_name, args) = split_name_eq(s, "--cryptol-arg-order")?;
            let parsed: Vec<CryArg> = args
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(parse_cry_arg)
                .collect();
            me.cryptol_arg_orders.insert(fn_name.to_string(), parsed);
        }
        // Cross-validate: every --cryptol-fn-out NAME must have a
        // matching --out-buffer-param NAME (otherwise there's no
        // allocated pointer to post-assert against).
        for (name, fn_) in &me.cryptol_fn_out {
            if !me.out_buffer_sizes.contains_key(name) {
                bail!("--cryptol-fn-out {name}={fn_}: no matching --out-buffer-param {name}=...");
            }
        }
        Ok(me)
    }

    /// SAW value-variable name for a C++ pointer parameter as bound
    /// by the emitter. Out-buffer params are renamed to `<name>_pre`
    /// to make the "this is the pre-call state, not the final value"
    /// intent obvious at every use site.
    pub fn value_var_for(&self, param_name: &str) -> String {
        if self.out_buffer_sizes.contains_key(param_name) {
            format!("{param_name}_pre")
        } else {
            param_name.to_string()
        }
    }

    /// Resolve a single `--cryptol-arg-order` token into the SAW
    /// variable name to splice into the generated `{{ ... }}` block.
    pub fn render_cry_arg(&self, arg: &CryArg) -> String {
        match arg {
            CryArg::Param(name) => self.value_var_for(name),
            CryArg::PreParam(name) => format!("{name}_pre"),
        }
    }

    /// If `--cryptol-arg-order` was supplied for `fn_name`, return
    /// the SAW variable names to splice into the Cryptol call;
    /// otherwise None (caller falls back to its positional default).
    pub fn cryptol_call_args(&self, fn_name: &str) -> Option<Vec<String>> {
        self.cryptol_arg_orders
            .get(fn_name)
            .map(|args| args.iter().map(|a| self.render_cry_arg(a)).collect())
    }

    /// SAW type for a pointer parameter that this override knows
    /// about, or None if the parameter is not buffer-overridden.
    pub fn override_saw_type(&self, param_name: &str) -> Option<String> {
        if let Some(n) = self.in_buffer_sizes.get(param_name) {
            return Some(format!("llvm_array {n} (llvm_int 8)"));
        }
        if let Some(n) = self.out_buffer_sizes.get(param_name) {
            return Some(format!("llvm_array {n} (llvm_int 8)"));
        }
        None
    }

    /// Whether `param_name` is declared as an out-buffer.
    pub fn is_out_buffer(&self, param_name: &str) -> bool {
        self.out_buffer_sizes.contains_key(param_name)
    }

    /// Iterator over `(out_param_name, cryptol_fn)` pairs from
    /// `--cryptol-fn-out`.
    pub fn out_buffer_fns(&self) -> Vec<(&str, &str)> {
        let mut v: Vec<_> = self
            .cryptol_fn_out
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        v.sort_by_key(|(k, _)| k.to_string());
        v
    }

    /// Ordered max-len preconditions for deterministic emission.
    pub fn max_len_preconds_ordered(&self) -> &[(String, u64)] {
        &self.max_len_preconds
    }

    /// Whether any overrides are configured.
    pub fn is_empty(&self) -> bool {
        self.in_buffer_sizes.is_empty()
            && self.out_buffer_sizes.is_empty()
            && self.cryptol_fn_out.is_empty()
            && self.max_len_preconds.is_empty()
            && self.cryptol_arg_orders.is_empty()
    }
}

fn split_name_eq<'a>(s: &'a str, flag: &str) -> Result<(&'a str, &'a str)> {
    s.split_once('=')
        .ok_or_else(|| anyhow!("{flag} value '{s}': expected NAME=VALUE"))
        .map(|(k, v)| (k.trim(), v.trim()))
}

fn parse_cry_arg(tok: &str) -> CryArg {
    if let Some(name) = tok.strip_prefix("@pre.") {
        CryArg::PreParam(name.to_string())
    } else {
        CryArg::Param(tok.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_in_and_out_buffers() {
        let ov = BufferOverrides::from_cli(
            &["m=4".into(), "b=4".into()],
            &["out=10".into()],
            &["out=canonicalize_lp_post".into()],
            &["nm=4".into(), "nb=4".into()],
            &[
                "canonicalize_lp_ret=nm,nb".into(),
                "canonicalize_lp_post=nm,m,nb,b,@pre.out".into(),
            ],
        )
        .unwrap();

        assert_eq!(ov.in_buffer_sizes["m"], 4);
        assert_eq!(ov.out_buffer_sizes["out"], 10);
        assert_eq!(ov.cryptol_fn_out["out"], "canonicalize_lp_post");
        assert_eq!(
            ov.max_len_preconds,
            vec![("nm".into(), 4), ("nb".into(), 4)]
        );
        assert_eq!(ov.value_var_for("out"), "out_pre");
        assert_eq!(ov.value_var_for("m"), "m");

        let post_args = ov.cryptol_call_args("canonicalize_lp_post").unwrap();
        assert_eq!(post_args, vec!["nm", "m", "nb", "b", "out_pre"]);

        let ret_args = ov.cryptol_call_args("canonicalize_lp_ret").unwrap();
        assert_eq!(ret_args, vec!["nm", "nb"]);

        assert_eq!(
            ov.override_saw_type("m"),
            Some("llvm_array 4 (llvm_int 8)".into())
        );
        assert_eq!(
            ov.override_saw_type("out"),
            Some("llvm_array 10 (llvm_int 8)".into())
        );
        assert_eq!(ov.override_saw_type("nm"), None);
    }

    #[test]
    fn cryptol_fn_out_requires_matching_out_buffer_param() {
        let err = BufferOverrides::from_cli(&[], &[], &["out=foo".into()], &[], &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--out-buffer-param"), "msg = {msg}");
    }

    #[test]
    fn rejects_malformed_value() {
        let err = BufferOverrides::from_cli(&["m=nine".into()], &[], &[], &[], &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--in-buffer-size"), "msg = {msg}");
    }

    #[test]
    fn empty_when_no_flags() {
        let ov = BufferOverrides::from_cli(&[], &[], &[], &[], &[]).unwrap();
        assert!(ov.in_buffer_sizes.is_empty());
        assert!(ov.out_buffer_sizes.is_empty());
        assert!(ov.cryptol_fn_out.is_empty());
        assert!(ov.max_len_preconds.is_empty());
        assert!(ov.cryptol_arg_orders.is_empty());
        assert_eq!(ov.cryptol_call_args("anything"), None);
        assert_eq!(ov.override_saw_type("anything"), None);
        assert_eq!(ov.value_var_for("m"), "m");
    }
}
