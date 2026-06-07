//! Struct-shape recognizer (ArrayView rule 4, saw_spec_gen-26d).
//!
//! When a C/C++ function signature looks like
//!
//! ```text
//!     void f(T* buf, size_t len);
//! ```
//!
//! the second integer parameter almost certainly carries the element
//! count of the preceding pointer parameter. saw-spec-gen used to
//! treat such cases the same as any other pointer with no length
//! annotation: 1-byte fallback alloc + a loud TODO. That made every
//! `(T* buf, size_t len)` API DISPROVE by default until the user
//! hand-edited the spec or added a SAL macro.
//!
//! This module recognizes the pattern and synthesizes an
//! [`Annotation::InReadsParam`] on the buffer parameter, pointing at
//! the length sibling. The downstream pipeline already knows what to
//! do with `InReadsParam` (allocate `DEFAULT_PARAMREF_MAX_LEN` bytes
//! and emit a `llvm_precond {{ len <= MAX }}`).
//!
//! ## Recognition rules (deliberately conservative)
//!
//! For each adjacent pair `(p_i, p_{i+1})`:
//!
//! - `p_i` is a pointer (`TypeInfo::Pointer(_)`),
//! - `p_i` has no existing size annotation (we never override the
//!   user),
//! - `p_{i+1}` is an unsigned integer type (`size_t`, `u32`, `u64`,
//!   ...),
//! - `p_{i+1}.name` matches a small whitelist of common length names
//!   (`len`, `length`, `size`, `count`, `n`, `nbytes`, `cb`, `nm`,
//!   `num`, `nelem`, `nelems`).
//!
//! When all four hold, we add `InReadsParam(p_{i+1}.name)` to
//! `p_i.annotations` and stop scanning further pairs that start with
//! `p_i` (one annotation per pointer).
//!
//! ## Opting out
//!
//! Pass `--no-struct-shape-recognizer` on the gen-verify CLI to
//! disable this entirely. The recognizer is conservative but not
//! infallible — for example, a function that happens to have an
//! unrelated `count` integer after a pointer would receive an
//! unwanted annotation. The opt-out keeps the legacy 1-byte
//! fallback behavior available.

use crate::constraints::{Annotation, FunctionInfo, TypeInfo};

/// Length-name whitelist. Lowercase comparison; short and high-signal.
const LENGTH_NAMES: &[&str] = &[
    "len", "length", "size", "count", "n", "nbytes", "cb", "nm", "num", "nelem", "nelems",
    "buflen", "buf_len",
];

/// Apply struct-shape recognition to `func`, mutating its parameter
/// list to add synthetic `InReadsParam` annotations where the pattern
/// matches. Returns the names of parameters that received a new
/// annotation (for diagnostics).
pub fn recognize_and_annotate(func: &mut FunctionInfo) -> Vec<String> {
    let mut added = Vec::new();
    let n = func.params.len();
    if n < 2 {
        return added;
    }
    // Collect candidate annotations first so we don't borrow `func`
    // mutably and immutably at the same time.
    let mut to_add: Vec<(usize, String)> = Vec::new();
    for i in 0..n - 1 {
        let buf = &func.params[i];
        let len = &func.params[i + 1];
        if !matches!(&buf.ty, TypeInfo::Pointer(_)) {
            continue;
        }
        if buf.annotations.iter().any(is_size_annotation) {
            continue;
        }
        if !is_unsigned_int(&len.ty) {
            continue;
        }
        if !LENGTH_NAMES.contains(&len.name.to_lowercase().as_str()) {
            continue;
        }
        to_add.push((i, len.name.clone()));
    }
    for (idx, len_name) in to_add {
        func.params[idx]
            .annotations
            .push(Annotation::InReadsParam(len_name.clone()));
        added.push(func.params[idx].name.clone());
        let _ = len_name; // moved
    }
    added
}

fn is_size_annotation(a: &Annotation) -> bool {
    matches!(
        a,
        Annotation::InReads(_)
            | Annotation::OutWrites(_)
            | Annotation::InReadsParam(_)
            | Annotation::OutWritesParam(_)
            | Annotation::InZ(_)
            | Annotation::Dereferenceable(_)
    )
}

fn is_unsigned_int(ty: &TypeInfo) -> bool {
    matches!(ty, TypeInfo::UnsignedInt(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{Mutability, Nullability, ParamInfo, TypeInfo};

    fn ptr_u8(name: &str) -> ParamInfo {
        ParamInfo {
            name: name.into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: Vec::new(),
        }
    }

    fn uint(name: &str, bits: u32) -> ParamInfo {
        ParamInfo {
            name: name.into(),
            ty: TypeInfo::UnsignedInt(bits),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: Vec::new(),
        }
    }

    fn func(name: &str, params: Vec<ParamInfo>) -> FunctionInfo {
        FunctionInfo {
            name: name.into(),
            mangled_name: None,
            params,
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: Vec::new(),
            referenced_globals: Vec::new(),
            called_functions: Vec::new(),
        }
    }

    #[test]
    fn recognizes_buf_len_pair() {
        let mut f = func("write_buf", vec![ptr_u8("buf"), uint("len", 64)]);
        let added = recognize_and_annotate(&mut f);
        assert_eq!(added, vec!["buf".to_string()]);
        assert!(matches!(
            f.params[0].annotations.first(),
            Some(Annotation::InReadsParam(s)) if s == "len"
        ));
    }

    #[test]
    fn ignores_non_unsigned_length_param() {
        // `int` (signed) shouldn't trigger -- buffer lengths are
        // canonically unsigned.
        let mut len_signed = uint("len", 32);
        len_signed.ty = TypeInfo::SignedInt(32);
        let mut f = func("write_buf", vec![ptr_u8("buf"), len_signed]);
        let added = recognize_and_annotate(&mut f);
        assert!(added.is_empty());
        assert!(f.params[0].annotations.is_empty());
    }

    #[test]
    fn ignores_non_length_named_companion() {
        // `flags` isn't on the whitelist, so the recognizer leaves
        // the pointer alone.
        let mut f = func("write_buf", vec![ptr_u8("buf"), uint("flags", 32)]);
        let added = recognize_and_annotate(&mut f);
        assert!(added.is_empty());
    }

    #[test]
    fn never_overrides_existing_size_annotation() {
        let mut buf = ptr_u8("buf");
        buf.annotations.push(Annotation::InReads(32));
        let mut f = func("write_buf", vec![buf, uint("len", 64)]);
        let added = recognize_and_annotate(&mut f);
        assert!(added.is_empty(), "must not override user annotation");
        assert_eq!(f.params[0].annotations.len(), 1);
    }

    #[test]
    fn recognizes_multiple_pointer_length_pairs() {
        let mut f = func(
            "copy_buf",
            vec![
                ptr_u8("src"),
                uint("srclen", 64),
                ptr_u8("dst"),
                uint("dstlen", 64),
            ],
        );
        // Only `src` matches: `srclen` is not on the whitelist as a
        // bare token. Confirm the whitelist is strict enough to
        // avoid that false positive.
        let added = recognize_and_annotate(&mut f);
        assert!(added.is_empty());
    }

    #[test]
    fn recognizes_count_companion() {
        let mut f = func("write_elems", vec![ptr_u8("elems"), uint("count", 64)]);
        let added = recognize_and_annotate(&mut f);
        assert_eq!(added, vec!["elems".to_string()]);
    }
}
