//! Derives the [`SretPrestate`] strategy for sret-returning functions.
//!
//! Extracted from `gen_verify.rs` to keep it within the 500-line limit.

use crate::constraints::{FunctionInfo, SretPrestate, TypeInfo};
use crate::{clang_ast, saw_emit};
use std::path::Path;

/// Determine how to pass the sret pre-state to the Cryptol model.
///
/// Reads the Cryptol type signature to find the trailing parameter's
/// byte width `K`.  If `K` equals the total sret buffer size, returns
/// [`SretPrestate::FullBuffer`].  Otherwise walks the C++ return
/// struct's field layout to find a unique field of size `K` and
/// returns [`SretPrestate::Slice`] with the field's offset.
pub(crate) fn derive_sret_prestate(
    cryptol_spec: &Path,
    cryptol_fn: &str,
    target_fn: &FunctionInfo,
) -> SretPrestate {
    let cry_k =
        match saw_emit::cryptol_bridge::cryptol_prestate_byte_width(cryptol_spec, cryptol_fn) {
            Some(k) => k,
            None => return SretPrestate::FullBuffer,
        };
    let buf_size = match &target_fn.return_type {
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => *n,
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 0 => *size_bytes,
        _ => return SretPrestate::FullBuffer,
    };
    if cry_k == buf_size {
        return SretPrestate::FullBuffer;
    }
    // Walk the struct's fields to find one whose size matches K.
    let fields = match &target_fn.return_type {
        TypeInfo::Struct { fields, .. } => fields,
        _ => return SretPrestate::FullBuffer,
    };
    let mut matches = Vec::new();
    let mut offset = 0usize;
    for (fname, fty) in fields {
        if let Some((sz, align)) = clang_ast::cpp_type_size_align(fty) {
            let rem = offset % align;
            if rem != 0 {
                offset += align - rem;
            }
            if sz == cry_k {
                matches.push((fname.clone(), offset));
            }
            offset += sz;
        }
    }
    match matches.len() {
        1 => SretPrestate::Slice {
            take_bytes: cry_k,
            drop_bytes: matches[0].1,
        },
        0 => {
            eprintln!(
                "warning: Cryptol pre-state expects [{cry_k}][8] but no field \
                 of that size in return type; passing full buffer",
            );
            SretPrestate::FullBuffer
        }
        _ => {
            eprintln!(
                "warning: Cryptol pre-state expects [{cry_k}][8] — multiple \
                 fields match: {}; passing full buffer",
                matches
                    .iter()
                    .map(|(n, o)| format!("{n}@{o}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            SretPrestate::FullBuffer
        }
    }
}
