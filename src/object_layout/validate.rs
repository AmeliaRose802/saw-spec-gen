//! Fail-closed configuration checks and semantic precondition lowering.

use super::model::{field_key, FieldLayout, LayoutConfig, LayoutPlan, ObjectPlan, Projection};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;

#[path = "validate_preconditions.rs"]
mod preconditions;
#[path = "validate_shapes.rs"]
mod shapes;
use preconditions::{lower, references};
pub use shapes::validate_object;
#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;

/// Select a non-pointer semantic leaf of an already validated object.
/// LLVM selectors are expression templates, not suffixes. Parenthesize `var`
/// when passing a compound expression. Byte projection is little-endian only.
pub fn field_expr(object: &ObjectPlan, field: &FieldLayout, var: &str) -> String {
    if let Some(width) = field
        .bit_width
        .filter(|_| object.projection == Projection::Bytes)
    {
        let bits: usize = field
            .llvm_type
            .trim_start_matches('i')
            .parse()
            .expect("validated backing integer");
        let raw = if field.size == 1 {
            format!("({var} @ {})", field.offset)
        } else {
            format!(
                "(join (reverse (take`{{{}}} (drop`{{{}}} {var}))))",
                field.size, field.offset
            )
        };
        let raw = if field.size * 8 > bits {
            format!("(drop`{{{}}} {raw})", field.size * 8 - bits)
        } else {
            raw
        };
        return format!(
            "(drop`{{{}}} ({raw} >> ({} : [{bits}])))",
            bits - width,
            field.bit_offset.unwrap()
        );
    }
    if field.runtime && object.projection == Projection::Fields {
        return format!("layout_{}_{}", object.region, field_key(&field.path));
    }
    match object.projection {
        Projection::Fields => format!("{var}.{}", field_key(&field.path)),
        Projection::Llvm => object
            .selectors
            .get(&field.path)
            .expect("missing LLVM selector: validate_object must precede emission")
            .replace('$', var),
        Projection::Bytes if field.size == 1 => format!("({var} @ {})", field.offset),
        Projection::Bytes => format!(
            "(join (reverse (take`{{{}}} (drop`{{{}}} {var}))))",
            field.size, field.offset
        ),
    }
}

/// Finish an already validated plan without borrowing its owning overrides.
/// Originals remain in the manifest; the returned predicates are for emission.
/// Failed validation leaves the caller's plan unchanged.
pub fn finish(
    plan: &mut LayoutPlan,
    config: &LayoutConfig,
    raw_preconds: &[String],
    sret_assert: Option<usize>,
    alias_sizes: &[String],
) -> Result<Vec<String>> {
    let mut checked = plan.clone();
    check_config(&mut checked, config, sret_assert)?;
    shapes::check_alias_sizes(&mut checked, alias_sizes)?;
    let refs = references(&checked)?;
    let mut warnings = Vec::new();
    let lowered = raw_preconds
        .iter()
        .map(|raw| lower(raw, &refs, &mut warnings))
        .collect::<Result<Vec<_>>>()?;
    checked.semantic_preconditions = raw_preconds.to_vec();
    for warning in warnings {
        if !checked.warnings.contains(&warning) {
            checked.warnings.push(warning);
        }
    }
    *plan = checked;
    Ok(lowered)
}

fn check_config(plan: &mut LayoutPlan, config: &LayoutConfig, prefix: Option<usize>) -> Result<()> {
    for (name, object) in &plan.objects {
        ensure!(
            name == &object.region && shapes::is_path(name) && !name.contains(['.', '[']),
            "invalid or inconsistent object region {name:?}"
        );
        for path in &object.framed {
            ensure!(
                shapes::is_path(path) && object.layout.fields.iter().any(|f| covers(path, &f.path)),
                "unknown frame field {}.{path}; padding is not a semantic field",
                object.region
            );
        }
    }
    for (path, expected) in &config.offsets {
        let (object, leaf) = exact_field(plan, path)?;
        ensure!(
            leaf.offset == *expected,
            "offset assertion {path}={expected} disagrees with compiler offset {} for {}",
            leaf.offset,
            object.layout.source_type
        );
    }
    for (region, expected) in &config.alignments {
        let object = plan
            .objects
            .get(region)
            .with_context(|| format!("unknown alignment region {region}"))?;
        ensure!(
            object.layout.alignment == *expected,
            "alignment assertion {region}={expected} disagrees with compiler alignment {}",
            object.layout.alignment
        );
    }
    super::config_keys::validate_active_members(config, plan)?;
    if let Some(prefix) = prefix {
        let object = plan
            .objects
            .get("return")
            .context("sret_assert_bytes requires a return object")?;
        ensure!(
            prefix <= object.layout.size,
            "sret_assert_bytes {prefix} exceeds compiler return size {}",
            object.layout.size
        );
        for field in &object.layout.fields {
            let end = field
                .offset
                .checked_add(field.size)
                .context("return field extent overflow")?;
            ensure!(
                end <= prefix,
                "sret_assert_bytes {prefix} omits semantic field return.{} (ends at {end})",
                field.path
            );
        }
    }
    for selection in config.modifies.iter().flatten() {
        let (region, path) = selection
            .split_once('.')
            .context("modifies requires a canonical region.field path")?;
        ensure!(
            region != "return",
            "modifies cannot select return storage: {selection}"
        );
        ensure!(shapes::is_path(path), "invalid modifies path {selection}");
        let object = plan
            .objects
            .get(region)
            .with_context(|| format!("unknown modifies region {region}"))?;
        ensure!(
            object.mutable,
            "modifies crosses the readonly mutability boundary: {selection}"
        );
        let fields: Vec<_> = object
            .layout
            .fields
            .iter()
            .filter(|f| covers(path, &f.path))
            .collect();
        ensure!(!fields.is_empty(), "modifies {selection} matches no semantic fields; padding cannot be modified by a field selector");
        for field in fields {
            ensure!(
                !field.is_pointer,
                "unsupported pointer mutation: {}.{} selected by modifies {selection}",
                region,
                field.path
            );
        }
    }
    for object in plan.objects.values_mut() {
        if object.region == "return" {
            ensure!(
                !object.layout.fields.iter().any(|f| f.is_pointer),
                "pointer return fields are unsupported"
            );
            object.framed.clear();
            continue;
        }
        let caller_owned = !matches!(object.lowering.as_str(), "byval" | "indirect_by_value");
        let frames = object
            .layout
            .fields
            .iter()
            .filter(|field| {
                field.is_pointer
                    || field
                        .validity
                        .as_deref()
                        .is_some_and(|v| v.starts_with("active_variant:"))
                    || (object.mutable
                        && caller_owned
                        && config.modifies.as_ref().map_or_else(
                            || object.framed.iter().any(|p| covers(p, &field.path)),
                            |modifies| {
                                !modifies.iter().any(|p| {
                                    covers(p, &format!("{}.{}", object.region, field.path))
                                })
                            },
                        ))
            })
            .map(|field| field.path.clone())
            .collect::<BTreeSet<_>>();
        object.framed = frames.into_iter().collect();
    }
    Ok(())
}

fn covers(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with(['.', '[']))
}

fn exact_field<'a>(plan: &'a LayoutPlan, path: &str) -> Result<(&'a ObjectPlan, &'a FieldLayout)> {
    let (region, relative) = path
        .split_once('.')
        .context("expected a canonical region.field path")?;
    let object = plan
        .objects
        .get(region)
        .with_context(|| format!("unknown field region {region} in {path}"))?;
    let fields: Vec<_> = object
        .layout
        .fields
        .iter()
        .filter(|f| f.path == relative)
        .collect();
    ensure!(fields.len() == 1, "unknown or ambiguous semantic leaf {path}; aggregate/padding offsets are not field assertions");
    Ok((object, fields[0]))
}
