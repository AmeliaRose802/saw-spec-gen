//! Semantic leaf checks, including bitfields that share an integer backing word.

use super::{MAX_DEPTH, MAX_ITEMS};
use crate::llvm_ir::IrStructDef;
use crate::object_layout::{data_layout::DataLayout, field_key, FieldLayout, ObjectPlan};
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[path = "validate_field_names.rs"]
mod names;
pub(crate) use names::{check_alias_sizes, guards, is_path};

pub(super) fn check_fields(
    object: &ObjectPlan,
    dl: &DataLayout,
    defs: &HashMap<String, IrStructDef>,
) -> Result<()> {
    let layout = &object.layout;
    ensure!(
        layout.unresolved.is_empty(),
        "unresolved layout for {}: {}",
        object.region,
        layout.unresolved.join("; ")
    );
    ensure!(
        layout.size > 0
            && layout.alignment.is_power_of_two()
            && layout.llvm_alignment.is_power_of_two()
            && layout.llvm_alignment <= layout.alignment
            && layout.size % layout.alignment == 0,
        "invalid compiler size/alignment for {}",
        object.region
    );
    ensure!(
        layout.fields.len() <= MAX_ITEMS,
        "semantic field limit exceeded"
    );
    let (mut paths, mut keys) = (BTreeSet::new(), BTreeMap::new());
    let mut ranges = Vec::new();
    for field in &layout.fields {
        ensure!(
            is_path(&field.path),
            "unresolved or invalid semantic field path {:?}",
            field.path
        );
        ensure!(
            !field.source_type.trim().is_empty(),
            "unresolved source type for {}.{}",
            object.region,
            field.path
        );
        ensure!(
            paths.insert(field.path.as_str()),
            "duplicate semantic field path {}.{}",
            object.region,
            field.path
        );
        let key = field_key(&field.path);
        if let Some(previous) = keys.insert(key.clone(), &field.path) {
            bail!(
                "field_key collision in {}: {previous} and {} both map to {key}",
                object.region,
                field.path
            );
        }
        let end = check_scalar(object, field, dl, defs)?;
        ranges.push((field, end));
        guards(object, field)?;
    }
    for path in &paths {
        for (index, _) in path
            .char_indices()
            .filter(|(_, ch)| matches!(ch, '.' | '['))
        {
            ensure!(
                !paths.contains(&path[..index]),
                "semantic path is both scalar and aggregate: {path}"
            );
        }
    }
    ranges.sort_by_key(|(f, _)| (f.offset, f.bit_offset.unwrap_or(0), &f.path));
    for pair in ranges.windows(2) {
        let (a, b) = (pair[0].0, pair[1].0);
        if pair[0].1 <= b.offset {
            continue;
        }
        let same_word =
            a.offset == b.offset && a.size == b.size && a.llvm_type.trim() == b.llvm_type.trim();
        let disjoint = match (a.bit_offset, a.bit_width, b.bit_offset, b.bit_width) {
            (Some(start), Some(width), Some(next), Some(_)) => start + width <= next,
            _ => false,
        };
        ensure!(
            same_word && disjoint,
            "overlapping semantic fields {} and {}",
            a.path,
            b.path
        );
        ensure!(
            a.guard == b.guard,
            "bitfield storage guards disagree for {} and {}",
            a.path,
            b.path
        );
    }
    for padding in &layout.padding {
        let end = padding
            .offset
            .checked_add(padding.size)
            .context("padding extent overflow")?;
        ensure!(
            end <= layout.size,
            "padding extends beyond compiler object size"
        );
        ensure!(
            padding.size == 0
                || !ranges
                    .iter()
                    .any(|(field, stop)| padding.offset < *stop && field.offset < end),
            "padding overlaps a semantic field backing range in {}",
            object.region
        );
    }
    Ok(())
}

fn check_scalar(
    object: &ObjectPlan,
    field: &FieldLayout,
    dl: &DataLayout,
    defs: &HashMap<String, IrStructDef>,
) -> Result<usize> {
    let bitfield = check_bitfield(field, dl)?;
    let ty = field.llvm_type.trim();
    ensure!(
        field.runtime || (!float_type(ty) && !source_float(&field.source_type)),
        "typed floating-point semantic mapping is not implemented for {}.{} ({ty})",
        object.region,
        field.path
    );
    let pointer = ty.ends_with('*') || ty == "ptr" || ty.starts_with("ptr ");
    ensure!(
        field.is_pointer == pointer,
        "pointer/type mismatch for {}.{}: {ty}",
        object.region,
        field.path
    );
    let allocated = dl
        .layout_of(ty, defs)
        .with_context(|| format!("unresolved field {}.{}", object.region, field.path))?;
    let size = if pointer {
        let compact: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
        ensure!(
            !compact.contains("addrspace(")
                || compact
                    .rsplit_once("addrspace(")
                    .is_some_and(|(_, suffix)| suffix.starts_with("0)")),
            "nondefault pointer address space is unsupported: {ty}"
        );
        dl.pointer_size
    } else if field.runtime && float_type(ty) {
        // Runtime unions may use float storage solely to force ABI alignment.
        match ty {
            "double" => 8,
            "float" => 4,
            _ => bail!("unsupported runtime float storage {ty}"),
        }
    } else {
        integer_bits(ty)
            .with_context(|| format!("unsupported/unresolved semantic scalar {ty}"))?
            .div_ceil(8)
    };
    // For a bitfield this is the LLVM word's store size, NOT ceil(bit_width/8)
    // or sizeof(source_type). Several semantic values can share this extent.
    ensure!(
        size > 0 && field.size == size && size <= allocated.size,
        "semantic field {}.{} has {} bytes, but {ty} stores {size}",
        object.region,
        field.path,
        field.size
    );
    ensure!(
        bitfield || field.validity.as_deref() != Some("bool") || matches!(ty, "i1" | "i8"),
        "bool field {}.{} does not have compiler bool storage",
        object.region,
        field.path
    );
    let end = field
        .offset
        .checked_add(size)
        .context("semantic field extent overflow")?;
    ensure!(
        end <= object.layout.size,
        "semantic field {}.{} extends beyond compiler size {}",
        object.region,
        field.path,
        object.layout.size
    );
    Ok(end)
}

fn check_bitfield(field: &FieldLayout, dl: &DataLayout) -> Result<bool> {
    let (start, width) = match (field.bit_offset, field.bit_width) {
        (None, None) => return Ok(false),
        (Some(start), Some(width)) => (start, width),
        _ => bail!("incomplete bitfield metadata for {}", field.path),
    };
    ensure!(
        dl.little_endian,
        "bitfield {} requires little-endian bit placement",
        field.path
    );
    ensure!(
        !field.is_pointer
            && !source_pointer(&field.source_type)
            && !source_float(&field.source_type),
        "bitfield {} cannot have pointer or floating-point semantics",
        field.path
    );
    let bits = integer_bits(field.llvm_type.trim())
        .filter(|bits| *bits <= 128)
        .with_context(|| {
            format!(
                "bitfield {} requires integer backing storage i1..i128",
                field.path
            )
        })?;
    ensure!(
        width > 0 && width <= bits && start.checked_add(width).is_some_and(|end| end <= bits),
        "bitfield {} has an invalid nonzero bit span in {bits}-bit storage",
        field.path
    );
    ensure!(
        field.array_count.is_none() && field.array_stride.is_none(),
        "bitfield {} is not an array storage cell",
        field.path
    );
    Ok(true)
}

pub(super) fn integer_bits(ty: &str) -> Option<usize> {
    let digits = ty.strip_prefix('i')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|bits| (1..(1 << 23)).contains(bits))
}

fn float_type(ty: &str) -> bool {
    matches!(
        ty,
        "half" | "bfloat" | "float" | "double" | "x86_fp80" | "fp128" | "ppc_fp128"
    )
}

fn source_float(ty: &str) -> bool {
    let words = ty
        .split_whitespace()
        .filter(|w| !matches!(*w, "const" | "volatile" | "mutable"))
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        words.as_str(),
        "float"
            | "double"
            | "long double"
            | "_Float16"
            | "_Float32"
            | "_Float64"
            | "_Float128"
            | "_Float32x"
            | "_Float64x"
            | "_Float128x"
            | "__float128"
            | "__fp16"
            | "__bf16"
            | "f16"
            | "f32"
            | "f64"
            | "f128"
    )
}

fn source_pointer(ty: &str) -> bool {
    let mut depth = 0usize;
    for ch in ty.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            '*' | '&' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}
