//! Named bitvectors share one typed LLVM cell; compiler bit padding is not a field.

use crate::object_layout::{
    validate::field_expr as scalar_expr, FieldLayout, ObjectPlan, Projection,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

struct Storage<'a> {
    first: &'a FieldLayout,
    fields: Vec<&'a FieldLayout>,
    bits: usize,
    named_mask: u128,
}

fn storage_bits(field: &FieldLayout) -> usize {
    let bits = field
        .llvm_type
        .trim()
        .strip_prefix('i')
        .and_then(|n| n.parse::<usize>().ok())
        .expect("validated integer bitfield storage");
    let width = field.bit_width.expect("validated bitfield width");
    let offset = field.bit_offset.expect("validated bitfield offset");
    assert!(
        (1..=128).contains(&bits)
            && (1..=bits).contains(&width)
            && offset.checked_add(width).is_some_and(|end| end <= bits)
            && field.size == bits.div_ceil(8)
            && !field.is_pointer,
        "validate_object must check bitfield storage before emission"
    );
    bits
}

fn ones(bits: usize) -> u128 {
    u128::MAX >> (128 - bits)
}

fn groups(object: &ObjectPlan) -> BTreeMap<usize, Storage<'_>> {
    let mut groups = BTreeMap::<usize, Storage<'_>>::new();
    for field in &object.layout.fields {
        if field.bit_width.is_none() {
            assert!(field.bit_offset.is_none(), "incomplete bitfield metadata");
            continue;
        }
        assert!(
            object.projection != Projection::Llvm,
            "legacy LLVM bitfield projection"
        );
        let bits = storage_bits(field);
        let mask = ones(field.bit_width.unwrap()) << field.bit_offset.unwrap();
        let group = groups.entry(field.offset).or_insert_with(|| Storage {
            first: field,
            fields: Vec::new(),
            bits,
            named_mask: 0,
        });
        assert!(
            group.bits == bits
                && group.first.size == field.size
                && group.first.guard == field.guard
                && group.named_mask & mask == 0,
            "validate_object must check shared bitfield ranges and guards"
        );
        group.named_mask |= mask;
        group.fields.push(field);
    }
    groups
}

fn value_name(object: &ObjectPlan) -> String {
    if object.region == "return" {
        "result_pre".into()
    } else if object.mutable {
        format!("{}_pre", object.region)
    } else {
        object.region.clone()
    }
}

fn storage_var(object: &ObjectPlan, offset: usize, suffix: &str) -> String {
    // A field_key cannot start with a digit, so these cannot shadow field vars.
    format!("layout_{}_0_bitfield_{offset}_{suffix}", object.region)
}

fn address(object: &ObjectPlan, offset: usize) -> String {
    let ptr = if object.region == "return" {
        "result_ptr".into()
    } else {
        format!("{}_ptr", object.region)
    };
    format!(
        "(llvm_elem (llvm_cast_pointer {ptr} (llvm_array {} (llvm_int 8))) {offset})",
        object.layout.size
    )
}

fn extract(field: &FieldLayout, word: &str) -> String {
    let bits = storage_bits(field);
    // Signed bitfields are still width-sized bitvectors, never sign-extended
    // backing words. Cryptol drop keeps the low bits after a logical shift.
    format!(
        "(drop`{{{}}} ({word} >> ({} : [{bits}])))",
        bits - field.bit_width.unwrap(),
        field.bit_offset.unwrap()
    )
}

fn byte_word(field: &FieldLayout, var: &str) -> String {
    let word = if field.size == 1 {
        format!("({var} @ {})", field.offset)
    } else {
        format!(
            "(join (reverse (take`{{{}}} (drop`{{{}}} {var}))))",
            field.size, field.offset
        )
    };
    let extra = field.size * 8 - storage_bits(field);
    if extra == 0 {
        word
    } else {
        format!("(drop`{{{extra}}} {word})")
    }
}

fn field_expr(object: &ObjectPlan, field: &FieldLayout, var: &str) -> String {
    if field.bit_width.is_some() && object.projection == Projection::Bytes {
        extract(field, &byte_word(field, var))
    } else {
        scalar_expr(object, field, var)
    }
}

fn guard(object: &ObjectPlan, field: &FieldLayout, var: &str) -> Option<String> {
    let mut pending: Vec<_> = field.guard.as_ref()?.split("&&").map(str::trim).collect();
    let mut flags = BTreeMap::new();
    while let Some(path) = pending.pop() {
        if flags.contains_key(path) {
            continue;
        }
        let flag = object
            .layout
            .fields
            .iter()
            .find(|f| f.path == path)
            .expect("validated optional guard");
        flags.insert(path, flag);
        if let Some(outer) = &flag.guard {
            pending.extend(outer.split("&&").map(str::trim));
        }
    }
    Some(
        flags
            .values()
            .map(|flag| format!("({} == 1)", field_expr(object, flag, var)))
            .collect::<Vec<_>>()
            .join(" && "),
    )
}

fn points_to(
    out: &mut String,
    object: &ObjectPlan,
    group: &Storage<'_>,
    value: &str,
    condition: Option<&str>,
) {
    let ptr = address(object, group.first.offset);
    let bits = group.bits;
    if let Some(condition) = condition {
        let _ = writeln!(out, "    llvm_conditional_points_to_at_type {{{{ {condition} }}}} {ptr} (llvm_int {bits}) ({value});");
    } else {
        let _ = writeln!(
            out,
            "    llvm_points_to_at_type {ptr} (llvm_int {bits}) ({value});"
        );
    }
}

/// Called after the main byte/record prevalue exists. In Fields projection,
/// its named bitfield terms must already have type [bit_width], not [storage].
/// This adds no allocation and must replace, not accompany, per-bitfield stores.
pub fn setup(out: &mut String, object: &ObjectPlan) {
    let before = value_name(object);
    for group in groups(object).into_values() {
        let condition = if object.region == "return" {
            None // Raw return storage has no live optional object yet.
        } else {
            guard(object, group.first, &before)
        };
        let word = if object.projection == Projection::Bytes {
            byte_word(group.first, &before)
        } else {
            let raw = storage_var(object, group.first.offset, "padding");
            let bits = group.bits;
            let _ = writeln!(
                out,
                "    // Compiler bit padding stays arbitrary; named fields share one typed word."
            );
            let _ = writeln!(
                out,
                "    {raw} <- llvm_fresh_var \"{raw}\" (llvm_int {bits});"
            );
            let mut parts = vec![format!(
                "({raw} && ({} : [{bits}]))",
                ones(bits) ^ group.named_mask
            )];
            for field in &group.fields {
                let value = field_expr(object, field, &before);
                parts.push(format!(
                    "((zero # ({value}) : [{bits}]) << ({} : [{bits}]))",
                    field.bit_offset.unwrap()
                ));
            }
            parts.join(" || ")
        };
        points_to(
            out,
            object,
            &group,
            &format!("llvm_term {{{{ {word} }}}}"),
            condition.as_deref(),
        );
        for field in &group.fields {
            if field.validity.is_some()
                && object.region != "return"
                && !(field.validity.as_deref() == Some("bool") && field.bit_width == Some(1))
            {
                let Some(valid) = crate::object_layout::enums::predicate(
                    field,
                    &field_expr(object, field, &before),
                ) else {
                    continue;
                };
                let predicate = guarded(condition.as_deref(), &valid);
                let _ = writeln!(
                    out,
                    "    // C++ valid bool bitfield: {}.{}",
                    object.region, field.path
                );
                let _ = writeln!(out, "    llvm_precond {{{{ {predicate} }}}};");
            }
        }
    }
}

fn guarded(condition: Option<&str>, predicate: &str) -> String {
    match condition {
        Some(condition) => format!("if {condition} then {predicate} else True"),
        None => predicate.into(),
    }
}

/// Bind one post-execution word, then constrain only named asserted/framed bits.
/// The post fresh variable matches actual memory; padding may change freely.
pub fn postcondition(out: &mut String, object: &ObjectPlan, model: Option<&str>) {
    let before = value_name(object);
    let expected = model.map(|call| format!("({call})"));
    for group in groups(object).into_values() {
        let mut obligations = Vec::new();
        for field in &group.fields {
            let framed = object.framed.contains(&field.path);
            let asserted = model.is_some() && object.asserted.contains(&field.path);
            if framed || asserted {
                let var = if asserted {
                    expected.as_deref().unwrap()
                } else {
                    &before
                };
                obligations.push((*field, framed, asserted, var, guard(object, field, var)));
            }
        }
        if obligations.is_empty() {
            continue;
        }
        // Mixed frame/model obligations can have different pre/post engagement
        // values despite identical guard paths. Read once under their union.
        let read_guard = if obligations.iter().any(|o| o.4.is_none()) {
            None
        } else {
            let guards: BTreeSet<_> = obligations.iter().filter_map(|o| o.4.as_deref()).collect();
            Some(if guards.len() == 1 {
                guards.into_iter().next().unwrap().to_owned()
            } else {
                guards
                    .into_iter()
                    .map(|g| format!("({g})"))
                    .collect::<Vec<_>>()
                    .join(" || ")
            })
        };
        let actual = storage_var(object, group.first.offset, "post");
        let _ = writeln!(
            out,
            "    {actual} <- llvm_fresh_var \"{actual}\" (llvm_int {});",
            group.bits
        );
        points_to(
            out,
            object,
            &group,
            &format!("llvm_term {actual}"),
            read_guard.as_deref(),
        );
        for (field, framed, asserted, var, condition) in obligations {
            let value = field_expr(object, field, var);
            let equality = format!("{} == {value}", extract(field, &actual));
            let predicate = guarded(condition.as_deref(), &equality);
            let _ = writeln!(
                out,
                "    // {} bitfield {}.{}",
                if asserted { "Assert" } else { "Frame" },
                object.region,
                field.path
            );
            let _ = writeln!(out, "    llvm_postcond {{{{ {predicate} }}}};");
            if framed && asserted {
                let equality = format!("{value} == {}", field_expr(object, field, &before));
                let predicate = guarded(condition.as_deref(), &equality);
                let _ = writeln!(
                    out,
                    "    llvm_postcond {{{{ {predicate} }}}}; // unchanged field"
                );
            }
        }
    }
}
