//! Field-level pre/post state assertions for verifying **stateful**
//! C++ methods (the R3 gap — see `docs/03-stateful-method-specs.md`).
//!
//! A stateful method's headline safety property is relational over the
//! pre/post heap (e.g. *"`activate` never turns an already-Active key
//! back to not-active"*). The pure functional-equivalence specs this
//! tool emits by default (`f(x) == model(x)`) cannot express that: they
//! treat `this` as an opaque pointer with no heap post-conditions.
//!
//! This module models a tracked object field as a fixed byte range
//! within the object and lets the user assert its value before and
//! after the call. The object itself is allocated as a byte array
//! (reusing the `--out-buffer-param NAME=BYTES` machinery), so the
//! assertions are plain Cryptol slices of the pre/post byte vectors —
//! no STL field-name resolution required.
//!
//! CLI surface (one repeatable flag, `--state-field`):
//!
//! ```text
//! PARAM.FIELD@OFFSET:WIDTH=PRE->POST
//! ```
//!
//! * `PARAM` — pointer parameter holding the object (usually `this`).
//! * `FIELD` — human label for the field (documentation + grouping).
//! * `OFFSET` — byte offset of the field within the object.
//! * `WIDTH` — field width in bytes.
//! * `PRE` — Cryptol expression the field must equal *before* the
//!   call. Empty or `*` leaves the pre-state unconstrained.
//! * `POST` — Cryptol expression the field must equal *after* the
//!   call. Empty or `*` leaves it unconstrained; the literal `keep`
//!   asserts the field is unchanged (`post == pre`).
//!
//! Example (KeyStore P1 — *Active is irreversible*):
//!
//! ```text
//! --state-field this.isActive@8:1=0->1   # Provisional -> Active
//! --state-field this.isActive@8:1=1->1   # Active -> Active (no revert)
//! ```

use anyhow::{bail, Context, Result};

/// Post-state transition for a tracked field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatePost {
    /// The field must equal this Cryptol expression after the call.
    Equals(String),
    /// The field is unchanged: post-state byte(s) == pre-state byte(s).
    Keep,
}

/// One tracked object field plus its pre/post transition values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateField {
    /// Pointer parameter holding the object (e.g. `this`).
    pub param: String,
    /// Human label for the field (e.g. `isActive`).
    pub field: String,
    /// Byte offset of the field within the object.
    pub offset: usize,
    /// Field width in bytes.
    pub width: usize,
    /// Pre-call constraint, or `None` if unconstrained.
    pub pre: Option<String>,
    /// Post-call constraint, or `None` if unconstrained.
    pub post: Option<StatePost>,
}

/// A collection of tracked state fields parsed from `--state-field`.
#[derive(Debug, Default, Clone)]
pub struct StateModel {
    pub fields: Vec<StateField>,
}

impl StateField {
    /// Cryptol expression selecting this field's value out of a byte
    /// vector variable (`<param>_pre` / `<param>_post`), as a
    /// `[WIDTH*8]` little-endian integer.
    ///
    /// Width 1 uses the single-byte index operator `@`; wider fields
    /// reconstruct the LLVM little-endian layout via
    /// `join (reverse (buf @@ [off .. off+w-1]))`.
    pub fn slice_expr(&self, buf_var: &str) -> String {
        if self.width == 1 {
            format!("({buf_var} @ {})", self.offset)
        } else {
            let last = self.offset + self.width - 1;
            format!(
                "(join (reverse ({buf_var} @@ [{} .. {}])))",
                self.offset, last
            )
        }
    }
}

impl StateModel {
    /// Parse `--state-field` values into a [`StateModel`].
    pub fn from_cli(state_field: &[String]) -> Result<Self> {
        let mut fields = Vec::with_capacity(state_field.len());
        for s in state_field {
            fields.push(parse_state_field(s)?);
        }
        Ok(StateModel { fields })
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Distinct object-parameter names referenced by any field, in
    /// first-seen order (deterministic emission).
    pub fn params(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for f in &self.fields {
            if !seen.contains(&f.param.as_str()) {
                seen.push(f.param.as_str());
            }
        }
        seen
    }

    /// Minimum object size (bytes) implied by the declared fields of
    /// `param` — the largest `offset + width`. Used to size the
    /// byte-array allocation when the user doesn't pass an explicit
    /// `--out-buffer-param PARAM=BYTES`.
    pub fn min_size_for(&self, param: &str) -> usize {
        self.fields
            .iter()
            .filter(|f| f.param == param)
            .map(|f| f.offset + f.width)
            .max()
            .unwrap_or(0)
    }

    /// Emit `llvm_precond` clauses for every field that declares a
    /// pre-state value. References `<param>_pre` (bound by the
    /// out-buffer machinery before `llvm_execute_func`).
    pub fn emit_preconditions(&self, out: &mut String) {
        let pre_fields: Vec<&StateField> = self.fields.iter().filter(|f| f.pre.is_some()).collect();
        if pre_fields.is_empty() {
            return;
        }
        out.push_str("    // stateful: pre-state object field constraints\n");
        for f in pre_fields {
            let lhs = f.slice_expr(&format!("{}_pre", f.param));
            let rhs = f.pre.as_ref().unwrap();
            out.push_str(&format!(
                "    llvm_precond {{{{ {lhs} == {rhs} }}}};  // {}.{}\n",
                f.param, f.field,
            ));
        }
        out.push('\n');
    }

    /// Emit post-state field assertions. For each object parameter with
    /// at least one post-constrained field, binds a fresh
    /// `<param>_post` byte vector to the final memory of `<param>_ptr`,
    /// then `llvm_postcond`s each field slice. `saw_type_for` resolves
    /// the SAW allocation type for a parameter (the byte-array type
    /// declared via the out-buffer override).
    pub fn emit_postconditions(&self, out: &mut String, saw_type_for: &dyn Fn(&str) -> String) {
        for param in self.params() {
            let post_fields: Vec<&StateField> = self
                .fields
                .iter()
                .filter(|f| f.param == param && f.post.is_some())
                .collect();
            if post_fields.is_empty() {
                continue;
            }
            let saw_ty = saw_type_for(param);
            out.push_str(&format!(
                "    // stateful: post-state object field constraints for `{param}`\n"
            ));
            out.push_str(&format!(
                "    {param}_post <- llvm_fresh_var \"{param}_post\" ({saw_ty});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to {param}_ptr (llvm_term {param}_post);\n",
            ));
            for f in post_fields {
                let lhs = f.slice_expr(&format!("{}_post", f.param));
                let rhs = match f.post.as_ref().unwrap() {
                    StatePost::Equals(expr) => expr.clone(),
                    StatePost::Keep => f.slice_expr(&format!("{}_pre", f.param)),
                };
                out.push_str(&format!(
                    "    llvm_postcond {{{{ {lhs} == {rhs} }}}};  // {}.{}\n",
                    f.param, f.field,
                ));
            }
            out.push('\n');
        }
    }
}

/// Parse one `PARAM.FIELD@OFFSET:WIDTH=PRE->POST` token.
fn parse_state_field(s: &str) -> Result<StateField> {
    let (lhs, rhs) = s.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("--state-field '{s}': expected PARAM.FIELD@OFFSET:WIDTH=PRE->POST")
    })?;
    let (param_field, off_width) = lhs.split_once('@').ok_or_else(|| {
        anyhow::anyhow!("--state-field '{s}': missing '@OFFSET:WIDTH' after PARAM.FIELD")
    })?;
    let (param, field) = param_field.split_once('.').ok_or_else(|| {
        anyhow::anyhow!("--state-field '{s}': expected PARAM.FIELD before '@' (e.g. this.isActive)")
    })?;
    let (off_str, width_str) = off_width
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--state-field '{s}': expected OFFSET:WIDTH after '@'"))?;
    let offset: usize = off_str
        .trim()
        .parse()
        .with_context(|| format!("--state-field '{s}': OFFSET must be an unsigned integer"))?;
    let width: usize = width_str
        .trim()
        .parse()
        .with_context(|| format!("--state-field '{s}': WIDTH must be an unsigned integer"))?;
    if width == 0 {
        bail!("--state-field '{s}': WIDTH must be at least 1 byte");
    }
    let (pre_str, post_str) = rhs.split_once("->").ok_or_else(|| {
        anyhow::anyhow!("--state-field '{s}': expected PRE->POST (use '->' to separate)")
    })?;
    let pre = parse_value(pre_str);
    let post = match parse_value(post_str) {
        None => None,
        Some(v) if v == "keep" => Some(StatePost::Keep),
        Some(v) => Some(StatePost::Equals(v)),
    };
    let param = param.trim().to_string();
    let field = field.trim().to_string();
    if param.is_empty() || field.is_empty() {
        bail!("--state-field '{s}': PARAM and FIELD must be non-empty");
    }
    Ok(StateField {
        param,
        field,
        offset,
        width,
        pre,
        post,
    })
}

/// Normalise a PRE/POST side: empty or `*` means "unconstrained".
fn parse_value(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t == "*" {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
#[path = "state_fields_tests.rs"]
mod tests;
