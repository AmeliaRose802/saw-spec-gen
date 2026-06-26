//! Bind Cryptol-signature type variables (`{n}(fin n, n <= 64) =>
//! [n][8] -> ...`) to C++/Rust parameter lengths.
//!
//! Implements rule 1 of the ArrayView umbrella (`saw_spec_gen-4po`):
//! when the user attaches a Cryptol spec to a function, every
//! `[n][T]` parameter type carries the buffer-length truth. Without
//! this binding, the spec generator would have to fall back to a
//! 1-byte allocation for an un-annotated `uint8_t*` parameter and
//! the proof goes DISPROVED.
//!
//! ## Design contract
//!
//! - Input: a [`PolyCrySig`] (from
//!   [`crate::parsers::cryptol_poly_sig`]) and the function's C++/
//!   Rust parameter list.
//! - Output: zero or more [`LengthBinding`]s, one per C++/Rust
//!   parameter whose Cryptol counterpart is a `[n][T]` (or `[K][T]`)
//!   sequence.
//! - Pairing rule: positional. Cryptol param `i` binds to C++/Rust
//!   param `i`. When the arities disagree, we emit no bindings
//!   (degraded mode) and let the caller log a warning.
//! - Open-ended type variables (no `<= K` upper bound predicate in
//!   the constraint context) fall back to [`Self::DEFAULT_MAX`] and
//!   mark the binding [`LengthBinding::is_open_ended`] so the
//!   pipeline can emit a stderr warning.
//!
//! ## What this module deliberately does NOT do
//!
//! - It does not mutate any [`FunctionInfo`]. The caller decides
//!   whether to inject synthetic annotations or to set buffer sizes
//!   directly. See [`apply_to_function`] for the default policy.
//! - It does not validate element-type compatibility against the
//!   C++/Rust pointee. A Cryptol `[n][16]` paired with a `uint8_t*`
//!   produces a binding using the *Cryptol* element width — the
//!   caller decides whether to warn.

use crate::constraints::{Annotation, FunctionInfo, TypeInfo};
use crate::parsers::cryptol_poly_sig::{PolyCrySig, PolyCryType};

/// One length-binding decision for a single parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthBinding {
    /// Index of the C++/Rust parameter this binding applies to.
    pub param_idx: usize,
    /// Name of the C++/Rust parameter (carried for diagnostics; the
    /// emitter doesn't strictly need it because it has the
    /// `param_idx`, but the warning text reads better with the name).
    pub param_name: String,
    /// Cryptol type variable bound to this parameter's length
    /// (`"n"` for `[n][8]`). `None` for concrete-length params
    /// (`[16][8]`), where the length is just the literal.
    pub type_var: Option<String>,
    /// Maximum buffer length to allocate (in *elements*, not bytes).
    /// When `type_var` is `Some` and the variable carries an upper
    /// bound, this is that bound. When the variable is open-ended,
    /// this is [`LengthBinding::DEFAULT_MAX`]. For concrete-length
    /// params it is just the literal length.
    pub max_length: usize,
    /// Element width in bits, taken from the Cryptol type
    /// (`[n][8]` → 8, `[n][32]` → 32).
    pub elem_bits: u32,
    /// True when the Cryptol type variable had no upper bound
    /// predicate and we fell back to [`Self::DEFAULT_MAX`].
    pub is_open_ended: bool,
}

impl LengthBinding {
    /// Fallback element count used when the Cryptol type variable
    /// has no `n <= K` upper bound predicate. Matches
    /// [`crate::constraints::derive::DEFAULT_PARAMREF_MAX_LEN`].
    pub const DEFAULT_MAX: usize = 16;
}

/// Decide bindings for `func` from `sig`. Returns an empty vector
/// when the Cryptol arity does not match the function arity (the
/// caller is expected to log a warning and proceed without any
/// bindings — better than guessing).
pub fn bind_lengths(sig: &PolyCrySig, func: &FunctionInfo) -> Vec<LengthBinding> {
    if sig.params.len() != func.params.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (idx, (cry_ty, c_param)) in sig.params.iter().zip(&func.params).enumerate() {
        // Only bind for pointer params — scalar Bit / [N] / SeqBitvector
        // pairs are already handled by the existing scalar/value path.
        if !matches!(&c_param.ty, TypeInfo::Pointer(_)) {
            continue;
        }
        match cry_ty {
            PolyCryType::SeqVar { len_var, elem_bits } => {
                let (max_length, is_open_ended) = match sig.upper_bounds.get(len_var) {
                    Some(k) => (*k, false),
                    None => (LengthBinding::DEFAULT_MAX, true),
                };
                out.push(LengthBinding {
                    param_idx: idx,
                    param_name: c_param.name.clone(),
                    type_var: Some(len_var.clone()),
                    max_length,
                    elem_bits: *elem_bits as u32,
                    is_open_ended,
                });
            }
            PolyCryType::SeqBitvector { len, elem_bits } => {
                out.push(LengthBinding {
                    param_idx: idx,
                    param_name: c_param.name.clone(),
                    type_var: None,
                    max_length: *len,
                    elem_bits: *elem_bits as u32,
                    is_open_ended: false,
                });
            }
            // Bit / Bitvector / Other -- not a sized-buffer shape.
            _ => {}
        }
    }
    out
}

/// Apply each binding to the corresponding parameter of `func` by
/// inserting a synthetic [`Annotation::InReads`] with the binding's
/// `max_length`. This re-uses the existing derive.rs path:
/// `_In_reads_(N)` already produces an `llvm_array N (llvm_int 8)`
/// allocation and skips the loud TODO.
///
/// Open-ended bindings are still applied (using
/// [`LengthBinding::DEFAULT_MAX`]); the caller is expected to emit a
/// stderr warning describing the fallback so the user can add the
/// missing upper-bound predicate.
///
/// Returns the list of warnings (one per open-ended binding) so the
/// caller can render them in its preferred channel.
pub fn apply_to_function(func: &mut FunctionInfo, bindings: &[LengthBinding]) -> Vec<String> {
    let mut warnings = Vec::new();
    for b in bindings {
        if let Some(p) = func.params.get_mut(b.param_idx) {
            // Don't shadow an existing user annotation -- the source
            // SAL / sidecar wins.
            let already_sized = p.annotations.iter().any(|a| {
                matches!(
                    a,
                    Annotation::InReads(_)
                        | Annotation::OutWrites(_)
                        | Annotation::InReadsParam(_)
                        | Annotation::OutWritesParam(_)
                        | Annotation::InZ(_)
                )
            });
            if already_sized {
                continue;
            }
            p.annotations.push(Annotation::InReads(b.max_length));
            if b.is_open_ended {
                let var = b.type_var.as_deref().unwrap_or("?");
                warnings.push(format!(
                    "warning[saw-spec-gen]: Cryptol type variable `{var}` in the spec for \
                     `{fn_name}` has no upper-bound predicate (`{var} <= K`). Param \
                     `{pname}` will be allocated as `llvm_array {max} (llvm_int {bits})` \
                     (the default). Add `{var} <= K` to the Cryptol signature's \
                     constraint context to tighten the bound.",
                    fn_name = func.name,
                    pname = b.param_name,
                    max = b.max_length,
                    bits = b.elem_bits,
                ));
            }
        }
    }
    warnings
}

/// Infer `max_len_precond` entries — `(len_param_name, K)` — for the
/// scalar length parameter that the struct-shape recognizer paired
/// with a `[n][T]` buffer parameter.
///
/// For each binding produced by [`bind_lengths`] that carries a
/// *concrete* Cryptol upper bound `K` (`is_open_ended == false`), if
/// the matching buffer parameter also has a synthetic
/// [`Annotation::InReadsParam`] naming a sibling length parameter
/// (added earlier by
/// [`crate::constraints::struct_shape_recognizer::recognize_and_annotate`]),
/// emit `(len_name, K)`. The caller splices each pair into the
/// generated spec as `llvm_precond {{ len_name <= K }}`.
///
/// This makes `--max-len-precond` unnecessary whenever the Cryptol
/// signature already pins the buffer's upper bound and the C/Rust
/// signature follows the `(T* buf, size_t len)` shape. Open-ended
/// bindings (no `n <= K` predicate) are skipped — there is no sound
/// `K` to assert.
pub fn infer_len_preconds(sig: &PolyCrySig, func: &FunctionInfo) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for b in bind_lengths(sig, func) {
        if b.is_open_ended {
            continue;
        }
        let Some(p) = func.params.get(b.param_idx) else {
            continue;
        };
        for a in &p.annotations {
            if let Annotation::InReadsParam(len_name) = a {
                out.push((len_name.clone(), b.max_length as u64));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{Mutability, Nullability, ParamInfo};
    use crate::parsers::cryptol_poly_sig::parse_poly_signature_from_str;

    fn ptr_u8(name: &str) -> ParamInfo {
        ParamInfo {
            name: name.into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        }
    }

    fn func_with_params(name: &str, params: Vec<ParamInfo>) -> FunctionInfo {
        FunctionInfo {
            name: name.into(),
            mangled_name: None,
            params,
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    #[test]
    fn binds_shared_n_across_two_pointer_params() {
        let sig = parse_poly_signature_from_str(
            "xor_bytes : {n}(fin n, n <= 64) => [n][8] -> [n][8] -> [n][8]",
            "xor_bytes",
        )
        .unwrap();
        // Cryptol arity 2 (the third [n][8] is the return type).
        let func = func_with_params("xor_bytes", vec![ptr_u8("a"), ptr_u8("b")]);
        let bindings = bind_lengths(&sig, &func);
        assert_eq!(bindings.len(), 2);
        for b in &bindings {
            assert_eq!(b.type_var.as_deref(), Some("n"));
            assert_eq!(b.max_length, 64);
            assert_eq!(b.elem_bits, 8);
            assert!(!b.is_open_ended);
        }
        assert_eq!(bindings[0].param_name, "a");
        assert_eq!(bindings[1].param_name, "b");
    }

    #[test]
    fn binds_concrete_length_sequence() {
        let sig = parse_poly_signature_from_str("k : [16][8] -> [32]", "k").unwrap();
        let func = func_with_params("k", vec![ptr_u8("s")]);
        let bindings = bind_lengths(&sig, &func);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].type_var, None);
        assert_eq!(bindings[0].max_length, 16);
        assert_eq!(bindings[0].elem_bits, 8);
        assert!(!bindings[0].is_open_ended);
    }

    #[test]
    fn open_ended_type_var_uses_default_max_and_flags_warning() {
        let sig = parse_poly_signature_from_str("f : {n}(fin n) => [n][8] -> [32]", "f").unwrap();
        let mut func = func_with_params("f", vec![ptr_u8("buf")]);
        let bindings = bind_lengths(&sig, &func);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].max_length, LengthBinding::DEFAULT_MAX);
        assert!(bindings[0].is_open_ended);

        let warns = apply_to_function(&mut func, &bindings);
        assert_eq!(warns.len(), 1);
        let w = &warns[0];
        assert!(w.contains("warning[saw-spec-gen]"), "{w}");
        assert!(w.contains("`n`"), "{w}");
        assert!(w.contains("`buf`"), "{w}");
        assert!(w.contains("`n <= K`"), "{w}");
        // The synthetic annotation must have been added.
        assert_eq!(func.params[0].annotations.len(), 1);
        assert!(matches!(
            &func.params[0].annotations[0],
            Annotation::InReads(n) if *n == LengthBinding::DEFAULT_MAX,
        ));
    }

    #[test]
    fn arity_mismatch_skips_all_bindings() {
        let sig =
            parse_poly_signature_from_str("f : {n}(fin n, n <= 8) => [n][8] -> [32]", "f").unwrap();
        let func = func_with_params("f", vec![ptr_u8("a"), ptr_u8("b")]);
        let bindings = bind_lengths(&sig, &func);
        assert!(bindings.is_empty());
    }

    #[test]
    fn existing_user_annotation_wins() {
        let sig = parse_poly_signature_from_str("f : {n}(fin n, n <= 64) => [n][8] -> [32]", "f")
            .unwrap();
        let mut func = func_with_params("f", vec![ptr_u8("buf")]);
        func.params[0].annotations.push(Annotation::InReads(8));
        let bindings = bind_lengths(&sig, &func);
        assert_eq!(bindings.len(), 1);
        let warns = apply_to_function(&mut func, &bindings);
        assert!(warns.is_empty());
        // The existing _In_reads_(8) must be the only sizing annotation.
        let sized: Vec<_> = func.params[0]
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::InReads(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(sized, vec![8]);
    }

    #[test]
    fn non_pointer_params_are_skipped() {
        // `f : {n}(fin n, n <= 64) => [n][8] -> [32] -> [32]`
        // Two params: bound-len buffer + scalar int.
        let sig =
            parse_poly_signature_from_str("f : {n}(fin n, n <= 64) => [n][8] -> [32] -> [32]", "f")
                .unwrap();
        let mut func = func_with_params(
            "f",
            vec![
                ptr_u8("buf"),
                ParamInfo {
                    name: "tag".into(),
                    ty: TypeInfo::UnsignedInt(32),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                },
            ],
        );
        let bindings = bind_lengths(&sig, &func);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].param_idx, 0);
        let _ = apply_to_function(&mut func, &bindings);
        // Scalar param must not have an InReads injected.
        assert!(func.params[1].annotations.is_empty());
    }

    #[test]
    fn infers_len_precond_for_struct_shape_paired_buffer() {
        // `copy : {n}(fin n, n <= 4) => [n][8] -> [64] -> [32]`
        // C signature `(uint8_t* m, size_t nm)`. The struct-shape
        // recognizer pairs them, putting `InReadsParam("nm")` on `m`.
        let sig = parse_poly_signature_from_str(
            "copy : {n}(fin n, n <= 4) => [n][8] -> [64] -> [32]",
            "copy",
        )
        .unwrap();
        let mut func = func_with_params(
            "copy",
            vec![
                ptr_u8("m"),
                ParamInfo {
                    name: "nm".into(),
                    ty: TypeInfo::UnsignedInt(64),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                },
            ],
        );
        func.params[0]
            .annotations
            .push(Annotation::InReadsParam("nm".into()));
        let preconds = infer_len_preconds(&sig, &func);
        assert_eq!(preconds, vec![("nm".to_string(), 4)]);
    }

    #[test]
    fn open_ended_bound_yields_no_precond() {
        let sig =
            parse_poly_signature_from_str("f : {n}(fin n) => [n][8] -> [64] -> [32]", "f").unwrap();
        let mut func = func_with_params(
            "f",
            vec![
                ptr_u8("buf"),
                ParamInfo {
                    name: "len".into(),
                    ty: TypeInfo::UnsignedInt(64),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                },
            ],
        );
        func.params[0]
            .annotations
            .push(Annotation::InReadsParam("len".into()));
        // Open-ended `n` (no `<= K`) => no sound bound to assert.
        assert!(infer_len_preconds(&sig, &func).is_empty());
    }

    #[test]
    fn no_struct_shape_pairing_yields_no_precond() {
        // Bounded `n <= 8` but the buffer was never paired with a
        // length sibling (no InReadsParam), so nothing to assert on.
        let sig =
            parse_poly_signature_from_str("f : {n}(fin n, n <= 8) => [n][8] -> [32]", "f").unwrap();
        let func = func_with_params("f", vec![ptr_u8("buf")]);
        assert!(infer_len_preconds(&sig, &func).is_empty());
    }
}
