//! Per-target overrides for buffer-shape parameters and Cryptol arg ordering.
//!
//! Exposed via the `gen-verify` CLI flags:
//!   --in-buffer-size   NAME=SHAPE
//!   --out-buffer-param NAME=SHAPE|auto
//!   --cryptol-fn-out   OUT_PARAM=FN
//!   --cryptol-fn-pre   PRE_FN
//!   --max-len-precond  NAME=VAL
//!   --cryptol-arg-order FN=arg1,arg2,...
//!
//! SHAPE is one of `BYTES`, `iW`, `NxiW`, `struct:Name`,
//! `{f1,f2,...}`, or `<{f1,f2,...}>`. See `parse_buf_saw_type`.
//!
//! Motivation: targets whose C++ signature carries raw `uint8_t*`
//! pointers + a separate scalar return value need to be verified
//! against two Cryptol functions — one modelling the output buffer
//! post-state, one modelling the return value — and need explicit
//! buffer sizes (the AST has no `_Out_writes_(N)` annotation on the
//! production headers). Hand-written SAW specs used to fill the gap;
//! these flags let `gen-verify` emit the same spec automatically.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashMap, HashSet};

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
    /// `--in-buffer-size NAME=SHAPE` — pointer param NAME is a
    /// read-only input buffer. The map value is the resolved SAW
    /// allocation type (accepted SHAPEs: `BYTES`, `iW`, `NxiW`,
    /// `struct:Name`, `{f1,f2,...}`, `<{f1,f2,...}>`).
    pub in_buffers: HashMap<String, String>,

    /// `--out-buffer-param NAME=SHAPE` — pointer param NAME is a
    /// writable output buffer. The emitter:
    ///   - allocates `NAME_ptr` with `llvm_alloc (<saw_type>)`
    ///   - binds a fresh `NAME_pre` to its pre-call contents
    ///   - post-asserts the new contents via `--cryptol-fn-out NAME=FN`
    ///
    /// The map value is the resolved SAW allocation type. SHAPE may be
    /// `BYTES`, `iW`, `NxiW`, `struct:Name`, `{f1,f2,...}`, or
    /// `<{f1,f2,...}>`. See `parse_buf_saw_type`.
    pub out_buffers: HashMap<String, String>,
    /// `--out-buffer-param NAME=auto` — treat pointer param NAME as an
    /// out-buffer but keep its inferred SAW pointee type (no byte-size
    /// override). Useful for stateful `this` members where we want
    /// pre/post heap assertions over the object layout, not a raw byte
    /// array cast.
    pub out_buffer_auto: HashSet<String>,

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
    /// Optional Cryptol predicate emitted as an `llvm_precond` just
    /// before `llvm_execute_func`. This is a lightweight "model_pre"
    /// hook for stateful specs.
    pub cryptol_fn_pre: Option<String>,
}

impl BufferOverrides {
    pub fn from_cli(
        in_buffer_size: &[String],
        out_buffer_param: &[String],
        cryptol_fn_out: &[String],
        max_len_precond: &[String],
        cryptol_arg_order: &[String],
        cryptol_fn_pre: &[String],
    ) -> Result<Self> {
        let mut me = BufferOverrides::default();
        for s in in_buffer_size {
            let (k, v) = split_name_eq(s, "--in-buffer-size")?;
            let saw_ty = parse_buf_saw_type(v, "--in-buffer-size")?;
            me.in_buffers.insert(k.to_string(), saw_ty);
        }
        for s in out_buffer_param {
            let (k, v) = split_name_eq(s, "--out-buffer-param")?;
            if v.eq_ignore_ascii_case("auto") {
                me.out_buffer_auto.insert(k.to_string());
            } else {
                let saw_ty = parse_buf_saw_type(v, "--out-buffer-param")?;
                me.out_buffers.insert(k.to_string(), saw_ty);
            }
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
        match cryptol_fn_pre {
            [] => {}
            [one] => me.cryptol_fn_pre = Some(one.trim().to_string()),
            _ => bail!("--cryptol-fn-pre expects at most one function name"),
        }
        // Cross-validate: every --cryptol-fn-out NAME must have a
        // matching --out-buffer-param NAME (otherwise there's no
        // allocated pointer to post-assert against).
        for (name, fn_) in &me.cryptol_fn_out {
            if name != "this" && !me.is_out_buffer(name) {
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
        if self.is_out_buffer(param_name) {
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
        if let Some(ty) = self.in_buffers.get(param_name) {
            return Some(ty.clone());
        }
        if let Some(ty) = self.out_buffers.get(param_name) {
            return Some(ty.clone());
        }
        None
    }

    /// Whether `param_name` is declared as an out-buffer.
    pub fn is_out_buffer(&self, param_name: &str) -> bool {
        self.out_buffers.contains_key(param_name) || self.out_buffer_auto.contains(param_name)
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

    pub fn cryptol_fn_pre(&self) -> Option<&str> {
        self.cryptol_fn_pre.as_deref()
    }

    pub fn has_in_buffer_size(&self, param_name: &str) -> bool {
        self.in_buffers.contains_key(param_name)
    }

    pub fn has_auto_out_buffers(&self) -> bool {
        !self.out_buffer_auto.is_empty()
    }

    /// Whether any overrides are configured.
    pub fn is_empty(&self) -> bool {
        self.in_buffers.is_empty()
            && self.out_buffers.is_empty()
            && self.out_buffer_auto.is_empty()
            && self.cryptol_fn_out.is_empty()
            && self.max_len_preconds.is_empty()
            && self.cryptol_arg_orders.is_empty()
            && self.cryptol_fn_pre.is_none()
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

/// Resolve an `--in-buffer-size` / `--out-buffer-param` SHAPE value
/// into the SAW allocation type for the buffer. Accepted forms:
///
///   - `N`     → `llvm_array N (llvm_int 8)` — a byte buffer of N
///     bytes. Use for objects whose fields are all byte-granular
///     (`uint8_t` / `uint8_t[]`), which the compiled body accesses
///     byte-by-byte.
///   - `iW`    → `llvm_int W` — a single wide scalar field (e.g. a
///     bare `uint32_t` member). The body loads/stores it as one
///     i`W`; a byte-array allocation can't satisfy that access, so
///     the field must be typed explicitly.
///   - `NxiW`  → `llvm_array N (llvm_int W)` — a homogeneous array of
///     N wide fields (e.g. `uint32_t[4]` → `4xi32`).
///   - `struct:Name` → `llvm_struct "struct.Name"` — use the named
///     LLVM layout from the loaded module, including implicit padding.
///   - `{f1,f2,...}`   → `llvm_struct_type [ <f1>, <f2>, ... ]` — a
///     heterogeneous struct with natural alignment. Each field `fi`
///     is itself a SHAPE (`iW`, `NxiW`, or `BYTES`), so mixed-width
///     layouts like `{16xi8,i8}` (a 16-byte Uuid then a `bool`) are
///     expressible. Use when the pointee is a struct whose fields the
///     body loads/stores at differing widths — a flat byte array
///     can't satisfy those typed accesses.
///   - `<{f1,f2,...}>` → `llvm_packed_struct_type [ ... ]` — same as
///     above but packed (no inter-field padding), matching a
///     `#pragma pack` / `__attribute__((packed))` layout.
fn parse_buf_saw_type(value: &str, flag: &str) -> Result<String> {
    let value = value.trim();
    if let Some(name) = value.strip_prefix("struct:") {
        let llvm_name = ensure_llvm_struct_prefix(name, flag, value)?;
        return Ok(format!("llvm_struct \"{llvm_name}\""));
    }
    // Struct shapes must be checked before the `x`/`i` scalar forms:
    // a value like `<{16xi8,i8}>` contains an `x` that `split_once`
    // would otherwise mis-handle.
    if let Some(inner) = value.strip_prefix("<{").and_then(|s| s.strip_suffix("}>")) {
        let fields = parse_struct_fields(inner, flag, value)?;
        return Ok(format!("llvm_packed_struct_type [{}]", fields.join(", ")));
    }
    if let Some(inner) = value.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let fields = parse_struct_fields(inner, flag, value)?;
        return Ok(format!("llvm_struct_type [{}]", fields.join(", ")));
    }
    if let Some((n_str, elem)) = value.split_once('x') {
        let n: usize = n_str.trim().parse().map_err(|_| {
            anyhow!("{flag} value '{value}': expected <count>xi<width> (e.g. 4xi32)")
        })?;
        let width = parse_int_width(elem.trim(), flag, value)?;
        return Ok(format!("llvm_array {n} (llvm_int {width})"));
    }
    if let Some(w_str) = value.strip_prefix('i') {
        let width = parse_int_width_digits(w_str, flag, value)?;
        return Ok(format!("llvm_int {width}"));
    }
    let n: usize = value.parse().map_err(|_| {
        anyhow!(
            "{flag} value '{value}': expected <bytes>, i<width>, <count>xi<width>, struct:<Type>, or {{f1,f2,...}}"
        )
    })?;
    Ok(format!("llvm_array {n} (llvm_int 8)"))
}

fn ensure_llvm_struct_prefix(name: &str, flag: &str, value: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("{flag} value '{value}': struct:<Type> requires a non-empty LLVM type name");
    }
    if name.starts_with("struct.") || name.starts_with("class.") || name.starts_with("union.") {
        Ok(name.to_string())
    } else {
        Ok(format!("struct.{name}"))
    }
}

/// Parse the comma-separated field list inside a `{...}` / `<{...}>`
/// struct shape. Nested struct fields are not split-aware.
fn parse_struct_fields(inner: &str, flag: &str, value: &str) -> Result<Vec<String>> {
    let inner = inner.trim();
    if inner.is_empty() {
        bail!("{flag} value '{value}': struct shape must list at least one field");
    }
    inner
        .split(',')
        .map(|f| parse_buf_saw_type(f.trim(), flag))
        .collect()
}

/// Parse an `i<width>` element-type token (the part after the `x` in a
/// `NxiW` shape).
fn parse_int_width(elem: &str, flag: &str, value: &str) -> Result<u32> {
    let w_str = elem.strip_prefix('i').ok_or_else(|| {
        anyhow!("{flag} value '{value}': element type must be i<width> (e.g. i32)")
    })?;
    parse_int_width_digits(w_str, flag, value)
}

/// Parse the bare width digits of an integer type and reject zero.
fn parse_int_width_digits(w_str: &str, flag: &str, value: &str) -> Result<u32> {
    let width: u32 = w_str.parse().map_err(|_| {
        anyhow!("{flag} value '{value}': integer width must be a positive number (e.g. i32)")
    })?;
    if width == 0 {
        bail!("{flag} value '{value}': integer width must be greater than 0");
    }
    Ok(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_in_and_out_buffers() {
        let ov = BufferOverrides::from_cli(
            &["m=4".into(), "b=4".into()],
            &["out=10".into(), "this=auto".into()],
            &["out=canonicalize_lp_post".into()],
            &["nm=4".into(), "nb=4".into()],
            &[
                "canonicalize_lp_ret=nm,nb".into(),
                "canonicalize_lp_post=nm,m,nb,b,@pre.out".into(),
            ],
            &["canonicalize_lp_pre".into()],
        )
        .unwrap();

        assert_eq!(ov.in_buffers["m"], "llvm_array 4 (llvm_int 8)");
        assert_eq!(ov.out_buffers["out"], "llvm_array 10 (llvm_int 8)");
        assert!(ov.out_buffer_auto.contains("this"));
        assert_eq!(ov.cryptol_fn_out["out"], "canonicalize_lp_post");
        assert_eq!(ov.cryptol_fn_pre(), Some("canonicalize_lp_pre"));
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
        let err =
            BufferOverrides::from_cli(&[], &[], &["out=foo".into()], &[], &[], &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--out-buffer-param"), "msg = {msg}");
    }

    #[test]
    fn rejects_malformed_value() {
        let err =
            BufferOverrides::from_cli(&["m=nine".into()], &[], &[], &[], &[], &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--in-buffer-size"), "msg = {msg}");
    }

    #[test]
    fn empty_when_no_flags() {
        let ov = BufferOverrides::from_cli(&[], &[], &[], &[], &[], &[]).unwrap();
        assert!(ov.in_buffers.is_empty());
        assert!(ov.out_buffers.is_empty());
        assert!(ov.out_buffer_auto.is_empty());
        assert!(ov.cryptol_fn_out.is_empty());
        assert!(ov.max_len_preconds.is_empty());
        assert!(ov.cryptol_arg_orders.is_empty());
        assert!(ov.cryptol_fn_pre.is_none());
        assert_eq!(ov.cryptol_call_args("anything"), None);
        assert_eq!(ov.override_saw_type("anything"), None);
        assert_eq!(ov.value_var_for("m"), "m");
    }

    #[test]
    fn rejects_multiple_cryptol_fn_pre_flags() {
        let err =
            BufferOverrides::from_cli(&[], &[], &[], &[], &[], &["pre_a".into(), "pre_b".into()])
                .unwrap_err();
        assert!(
            format!("{err:#}").contains("--cryptol-fn-pre"),
            "msg = {err:#}"
        );
    }

    #[test]
    fn parses_typed_wide_buffer_shapes() {
        // iW form: a single wide scalar field allocates as llvm_int W.
        // NxiW form: a homogeneous array of wide fields.
        let ov = BufferOverrides::from_cli(
            &["hdr=2xi16".into()],
            &["n=i32".into(), "arr=4xi32".into()],
            &["n=inc_post".into(), "arr=arr_post".into()],
            &[],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(
            ov.override_saw_type("hdr"),
            Some("llvm_array 2 (llvm_int 16)".into())
        );
        assert_eq!(ov.override_saw_type("n"), Some("llvm_int 32".into()));
        assert_eq!(
            ov.override_saw_type("arr"),
            Some("llvm_array 4 (llvm_int 32)".into())
        );
        assert!(ov.is_out_buffer("n"));
        assert!(ov.is_out_buffer("arr"));
        assert!(ov.has_in_buffer_size("hdr"));
    }

    #[test]
    fn parses_struct_buffer_shapes() {
        let ov = BufferOverrides::from_cli(
            &["hdr=struct:PacketHeader".into()],
            &["key={16xi8,i8}".into(), "pk=<{i64,i8}>".into()],
            &["key=key_post".into(), "pk=pk_post".into()],
            &[],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(
            ov.override_saw_type("hdr"),
            Some("llvm_struct \"struct.PacketHeader\"".into())
        );
        assert_eq!(
            ov.override_saw_type("key"),
            Some("llvm_struct_type [llvm_array 16 (llvm_int 8), llvm_int 8]".into())
        );
        assert_eq!(
            ov.override_saw_type("pk"),
            Some("llvm_packed_struct_type [llvm_int 64, llvm_int 8]".into())
        );
        assert!(ov.is_out_buffer("key"));
        assert!(ov.is_out_buffer("pk"));
        assert!(ov.has_in_buffer_size("hdr"));
    }

    #[test]
    fn rejects_empty_struct_shape() {
        let err = BufferOverrides::from_cli(&["m={}".into()], &[], &[], &[], &[], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one field"),
            "msg = {err:#}"
        );
        let err =
            BufferOverrides::from_cli(&["m=struct:".into()], &[], &[], &[], &[], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("non-empty LLVM type name"),
            "msg = {err:#}"
        );
    }

    #[test]
    fn rejects_bad_typed_buffer_shapes() {
        // Missing `i` on the element width.
        let err =
            BufferOverrides::from_cli(&[], &["n=4x32".into()], &[], &[], &[], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("--out-buffer-param"),
            "msg = {err:#}"
        );
        // Zero width is rejected.
        let err = BufferOverrides::from_cli(&["m=i0".into()], &[], &[], &[], &[], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("greater than 0"),
            "msg = {err:#}"
        );
    }
}
