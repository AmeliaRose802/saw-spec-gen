//! Semantic names, transitive optional guards and compiler-grounded alias sizes.

use super::MAX_DEPTH;
use crate::object_layout::{
    clang::normalize_name, data_layout::DataLayout, FieldLayout, LayoutPlan, ObjectPlan,
};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) fn guards<'a>(
    object: &'a ObjectPlan,
    field: &'a FieldLayout,
) -> Result<Vec<&'a FieldLayout>> {
    fn visit<'a>(
        object: &'a ObjectPlan,
        field: &'a FieldLayout,
        active: &mut BTreeSet<&'a str>,
        result: &mut BTreeMap<&'a str, &'a FieldLayout>,
    ) -> Result<()> {
        ensure!(
            active.len() < MAX_DEPTH && active.insert(&field.path),
            "cyclic or excessively nested optional guards at {}",
            field.path
        );
        if let Some(guard) = &field.guard {
            for path in guard.split("&&").map(str::trim) {
                let candidates: Vec<_> = object
                    .layout
                    .fields
                    .iter()
                    .filter(|f| f.path == path)
                    .collect();
                ensure!(
                    candidates.len() == 1
                        && candidates[0].validity.as_deref() == Some("bool")
                        && !candidates[0].is_pointer,
                    "unknown/ambiguous/non-bool optional guard {path:?} for {}",
                    field.path
                );
                let flag = candidates[0];
                if !result.contains_key(flag.path.as_str()) {
                    visit(object, flag, active, result)?;
                    result.insert(&flag.path, flag);
                }
            }
        }
        active.remove(field.path.as_str());
        Ok(())
    }
    let mut result = BTreeMap::new();
    visit(object, field, &mut BTreeSet::new(), &mut result)?;
    Ok(result.into_values().collect())
}

pub(crate) fn is_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    loop {
        if !bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            return false;
        }
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            i += 1;
        }
        while bytes.get(i) == Some(&b'[') {
            let start = i + 1;
            i = start;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if start == i
                || bytes.get(i) != Some(&b']')
                || text[start..i].parse::<usize>().is_err()
                || (i - start > 1 && bytes[start] == b'0')
            {
                return false;
            }
            i += 1;
        }
        if i == bytes.len() {
            return true;
        }
        if bytes[i] != b'.' {
            return false;
        }
        i += 1;
    }
}

fn alias_key(name: &str) -> String {
    let name = name.trim().trim_start_matches('%').trim_matches('"');
    let name = ["struct.", "class.", "union."]
        .into_iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name);
    normalize_name(name)
}

pub(crate) fn check_alias_sizes(plan: &mut LayoutPlan, aliases: &[String]) -> Result<()> {
    let mut data_layout = None;
    for entry in aliases {
        let (name, bytes) = entry
            .split_once('=')
            .context("alias-size requires NAME=BYTES")?;
        ensure!(
            !name.trim().is_empty()
                && !bytes.trim().is_empty()
                && bytes.trim().bytes().all(|b| b.is_ascii_digit()),
            "invalid alias-size assertion {entry}"
        );
        let bytes: usize = bytes
            .trim()
            .parse()
            .context("alias-size byte count overflow")?;
        let key = alias_key(name);
        let mut known = Vec::new();
        for object in plan.objects.values() {
            if key == alias_key(&object.layout.source_type)
                || key == alias_key(&object.layout.llvm_type)
            {
                known.push((object.region.clone(), object.layout.size));
            }
        }
        if known.is_empty() {
            for object in plan.objects.values() {
                for field in &object.layout.fields {
                    // A bitfield's backing word is not sizeof its source typedef.
                    if field.bit_width.is_none() && key == alias_key(&field.source_type) {
                        if data_layout.is_none() {
                            data_layout = Some(DataLayout::parse(&plan.data_layout)?);
                        }
                        let dl = data_layout
                            .as_ref()
                            .context("missing alias assertion data layout")?;
                        let size = if field.is_pointer {
                            dl.pointer_layout(0)?.size
                        } else {
                            dl.layout_of(&field.llvm_type, &HashMap::new())?.size
                        };
                        known.push((format!("{}.{}", object.region, field.path), size));
                    }
                }
            }
        }
        for (region, actual) in &known {
            ensure!(
                bytes == *actual,
                "alias-size {entry} disagrees with exact compiler size {actual} for {region}"
            );
        }
        if known.is_empty() {
            let warning = format!("alias-size {entry}: unknown/non-target type; no compiler-grounded size assertion was made for this plan");
            if !plan.warnings.contains(&warning) {
                plan.warnings.push(warning);
            }
        }
    }
    Ok(())
}
