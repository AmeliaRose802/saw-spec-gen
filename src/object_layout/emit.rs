//! One typed allocation, with scalar cells constructed from its contract view.

use super::{field_key, validate::field_expr, FieldLayout, ObjectPlan, Projection};
use std::fmt::Write;

pub fn value_name(object: &ObjectPlan) -> String {
    if object.region == "return" {
        "result_pre".into()
    } else if object.mutable {
        format!("{}_pre", object.region)
    } else {
        object.region.clone()
    }
}

fn pointer_name(object: &ObjectPlan) -> String {
    if object.region == "return" {
        "result_ptr".into()
    } else {
        format!("{}_ptr", object.region)
    }
}

fn field_var(object: &ObjectPlan, field: &FieldLayout) -> String {
    format!("layout_{}_{}", object.region, field_key(&field.path))
}

fn saw_scalar(field: &FieldLayout) -> String {
    if field.is_pointer {
        "llvm_pointer (llvm_int 8)".into()
    } else if let Some(width) = field.bit_width {
        format!("llvm_int {width}")
    } else if field.runtime && matches!(field.llvm_type.as_str(), "float" | "double") {
        format!("llvm_int {}", field.size * 8)
    } else {
        format!("llvm_int {}", field.llvm_type.trim_start_matches('i'))
    }
}

/// Casts/element selection stay within the original exact-sized allocation.
fn address(object: &ObjectPlan, offset: usize) -> String {
    format!(
        "(llvm_elem (llvm_cast_pointer {} (llvm_array {} (llvm_int 8))) {offset})",
        pointer_name(object),
        object.layout.size
    )
}

fn guard(object: &ObjectPlan, field: &FieldLayout, var: &str) -> Option<String> {
    field.guard.as_ref().map(|guard| {
        guard
            .split(" && ")
            .map(|path| {
                let flag = object
                    .layout
                    .fields
                    .iter()
                    .find(|f| f.path == path)
                    .expect("validated optional guard");
                format!("({} == 1)", field_expr(object, flag, var))
            })
            .collect::<Vec<_>>()
            .join(" && ")
    })
}

fn points_to(
    out: &mut String,
    object: &ObjectPlan,
    field: &FieldLayout,
    value: &str,
    guard: Option<&str>,
) {
    let ptr = address(object, field.offset);
    let ty = saw_scalar(field);
    if let Some(condition) = guard {
        let _ = writeln!(
            out,
            "    llvm_conditional_points_to_at_type {{{{ {condition} }}}} {ptr} ({ty}) ({value});"
        );
    } else {
        let _ = writeln!(out, "    llvm_points_to_at_type {ptr} ({ty}) ({value});");
    }
}

pub fn setup(out: &mut String, object: &ObjectPlan) -> (String, String) {
    let ptr = pointer_name(object);
    let var = value_name(object);
    let allocation = if object.mutable {
        "llvm_alloc_aligned"
    } else {
        "llvm_alloc_readonly_aligned"
    };
    let _ = writeln!(
        out,
        "    // Validated {}: {} bytes, align {}; LLVM {}",
        object.region, object.layout.size, object.layout.alignment, object.layout.llvm_type
    );
    let allocation_type = if object.layout.allocation_type.is_empty() {
        format!("llvm_alias \"{}\"", object.layout.llvm_type)
    } else {
        object.layout.allocation_type.clone()
    };
    let _ = writeln!(
        out,
        "    {ptr} <- {allocation} {} ({allocation_type});",
        object.layout.alignment
    );
    match object.projection {
        Projection::Bytes => {
            let _ = writeln!(
                out,
                "    {var} <- llvm_fresh_var \"{var}\" (llvm_array {} (llvm_int 8));",
                object.layout.size
            );
        }
        Projection::Llvm => {
            let _ = writeln!(
                out,
                "    {var} <- llvm_fresh_var \"{var}\" ({});",
                object
                    .configured_shape
                    .as_deref()
                    .or(object.inferred_shape.as_deref())
                    .expect("validated shape")
            );
        }
        Projection::Fields => {
            let mut fields = Vec::new();
            for field in &object.layout.fields {
                let name = field_var(object, field);
                if field.is_pointer {
                    let _ = writeln!(out, "    {name} <- llvm_fresh_pointer (llvm_int 8);");
                } else {
                    let _ = writeln!(
                        out,
                        "    {name} <- llvm_fresh_var \"{name}\" ({});",
                        saw_scalar(field)
                    );
                    if !field.runtime {
                        fields.push(format!("{} = {name}", field_key(&field.path)));
                    }
                }
            }
            let _ = writeln!(
                out,
                "    let {var} = {{{{ {{ {} }} }}}};",
                fields.join(", ")
            );
        }
    }
    super::emit_bitfields::setup(out, object);
    for field in object
        .layout
        .fields
        .iter()
        .filter(|f| f.bit_width.is_none())
    {
        let value = if field.is_pointer {
            field_var(object, field)
        } else {
            format!("llvm_term {{{{ {} }}}}", field_expr(object, field, &var))
        };
        // Optional inactive payload storage is deliberately uninitialized. In a
        // raw sret buffer no C++ object exists yet, so seed unconstrained cells.
        let condition = if object.region == "return" {
            None
        } else {
            guard(object, field, &var)
        };
        points_to(out, object, field, &value, condition.as_deref());
        if field.validity.is_some() && object.region != "return" {
            let value = field_expr(object, field, &var);
            let Some(valid) = super::enums::predicate(field, &value) else {
                continue;
            };
            let predicate = match condition {
                Some(c) => format!("if {c} then {valid} else True"),
                None => valid,
            };
            let category = if field
                .validity
                .as_deref()
                .is_some_and(|v| v.starts_with("active_variant:"))
            {
                "User active-member scope"
            } else {
                "C++ valid representation"
            };
            let _ = writeln!(out, "    // {category}: {}.{}", object.region, field.path);
            let _ = writeln!(out, "    llvm_precond {{{{ {predicate} }}}};");
        }
    }
    for (i, padding) in object.layout.padding.iter().enumerate() {
        if padding.size == 0 {
            continue;
        }
        let _ = writeln!(
            out,
            "    // {}: bytes {}..{} (not post-asserted)",
            padding.reason,
            padding.offset,
            padding.offset + padding.size
        );
        let value = if object.projection == Projection::Bytes {
            format!(
                "take`{{{}}} (drop`{{{}}} {var})",
                padding.size, padding.offset
            )
        } else {
            let name = format!("layout_{}_padding_{i}", object.region);
            let _ = writeln!(
                out,
                "    {name} <- llvm_fresh_var \"{name}\" (llvm_array {} (llvm_int 8));",
                padding.size
            );
            name
        };
        let _ = writeln!(out, "    llvm_points_to_at_type {} (llvm_array {} (llvm_int 8)) (llvm_term {{{{ {value} }}}});",
            address(object, padding.offset), padding.size);
    }
    (var, ptr)
}

pub fn postcondition(out: &mut String, object: &ObjectPlan, model: Option<&str>) {
    super::emit_bitfields::postcondition(out, object, model);
    let before = value_name(object);
    let expected = model.map(|call| format!("({call})"));
    for field in object
        .layout
        .fields
        .iter()
        .filter(|f| f.bit_width.is_none())
    {
        let framed = object.framed.contains(&field.path);
        let asserted = model.is_some() && object.asserted.contains(&field.path);
        if !framed && !asserted {
            continue;
        }
        let var = if asserted {
            expected.as_deref().unwrap()
        } else {
            &before
        };
        let value = if field.is_pointer {
            field_var(object, field)
        } else {
            format!("llvm_term {{{{ {} }}}}", field_expr(object, field, var))
        };
        let condition = guard(object, field, var);
        let _ = writeln!(
            out,
            "    // {} {}.{}: byte {} + {}",
            if asserted { "Assert" } else { "Frame" },
            object.region,
            field.path,
            field.offset,
            field.size
        );
        points_to(out, object, field, &value, condition.as_deref());
        if object.region == "return"
            && field
                .validity
                .as_deref()
                .is_some_and(|v| v.starts_with("active_variant:"))
        {
            if let Some(predicate) = super::enums::predicate(field, &field_expr(object, field, var))
            {
                let _ = writeln!(
                    out,
                    "    llvm_postcond {{{{ {predicate} }}}}; // selected variant alternative"
                );
            }
        }
        if framed && asserted && !field.is_pointer {
            let equality = format!(
                "{} == {}",
                field_expr(object, field, var),
                field_expr(object, field, &before)
            );
            let predicate = condition
                .map(|c| format!("if {c} then {equality} else True"))
                .unwrap_or(equality);
            let _ = writeln!(
                out,
                "    llvm_postcond {{{{ {predicate} }}}}; // unchanged field"
            );
        }
    }
}
