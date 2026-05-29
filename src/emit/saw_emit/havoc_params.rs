//! Per-parameter setup + postcondition emitters used by
//! [`super::havoc::generate_havoc_spec`].

use super::havoc::HavocBehavior;
use crate::clang_ast::ClassConstructor;
use crate::constraints::*;

/// Emit setup + postconditions for a mutable `this` pointer when full
/// class layout is known. Pre-state allocates an anonymous packed
/// struct (vptr + data members + trailing padding), reads the vptr +
/// fields as fresh vars, then post-state rewrites every data member
/// with a fresh symbolic value while preserving the vptr.
///
/// An anonymous struct type is used (rather than `llvm_alias "class.X"`)
/// because passes like `opt -O1` strip named struct types from the
/// linked bitcode — `llvm_alias` would then fail to resolve at SAW
/// load time.
pub fn emit_this_full_class_havoc(
    layout: &ClassConstructor,
    setup: &mut String,
    postconds: &mut String,
) {
    let pad_bytes = trailing_padding_bytes(&layout.layout_fields);
    let has_pad = pad_bytes > 0;
    setup.push_str("\n    // Parameter: this (mutable → full-class HAVOC)\n");
    setup.push_str("    this_ptr <- llvm_alloc (llvm_packed_struct_type\n");
    setup.push_str("        [ llvm_pointer (llvm_int 64)  // vptr\n");
    for (_, fty, _) in &layout.layout_fields {
        let width = field_width(fty);
        setup.push_str(&format!("        , llvm_int {width}\n"));
    }
    if has_pad {
        setup.push_str(&format!("        , llvm_array {pad_bytes} (llvm_int 8)\n"));
    }
    setup.push_str("        ]);\n");
    setup.push_str("    this_vptr_pre <- llvm_alloc_readonly_aligned 8 (llvm_int 64);\n");
    for (fname, fty, _) in &layout.layout_fields {
        let width = field_width(fty);
        setup.push_str(&format!(
            "    this_{fname}_pre <- llvm_fresh_var \"this_{fname}_pre\" (llvm_int {width});\n",
        ));
    }
    if has_pad {
        setup.push_str(&format!(
            "    this_pad_pre <- llvm_fresh_var \"this_pad_pre\" (llvm_array {pad_bytes} (llvm_int 8));\n",
        ));
    }
    setup.push_str("    llvm_points_to this_ptr (llvm_packed_struct_value\n");
    setup.push_str("        [ this_vptr_pre  // vptr (any pointer)\n");
    for (fname, _, _) in &layout.layout_fields {
        setup.push_str(&format!("        , llvm_term this_{fname}_pre\n"));
    }
    if has_pad {
        setup.push_str("        , llvm_term this_pad_pre\n");
    }
    setup.push_str("        ]);\n");

    postconds.push_str("    // this: HAVOCED — data members may have changed\n");
    for (fname, fty, _) in &layout.layout_fields {
        let width = field_width(fty);
        postconds.push_str(&format!(
            "    this_{fname}_post <- llvm_fresh_var \"this_{fname}_post\" (llvm_int {width});\n",
        ));
    }
    if has_pad {
        postconds.push_str(&format!(
            "    this_pad_post <- llvm_fresh_var \"this_pad_post\" (llvm_array {pad_bytes} (llvm_int 8));\n",
        ));
    }
    postconds.push_str("    llvm_points_to this_ptr (llvm_packed_struct_value\n");
    postconds.push_str("        [ this_vptr_pre  // vptr preserved\n");
    for (fname, _, _) in &layout.layout_fields {
        postconds.push_str(&format!("        , llvm_term this_{fname}_post\n"));
    }
    if has_pad {
        postconds.push_str("        , llvm_term this_pad_post\n");
    }
    postconds.push_str("        ]);\n");
}

fn field_width(fty: &TypeInfo) -> u32 {
    match fty {
        TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => *w,
        TypeInfo::Bool => 1,
        _ => 64,
    }
}

/// Compute trailing padding (in bytes) needed to round the packed
/// struct (8-byte vptr + data members) up to an 8-byte boundary,
/// matching clang's class layout for an x86_64 polymorphic class.
fn trailing_padding_bytes(fields: &[(String, TypeInfo, String)]) -> u32 {
    let mut bits = 64u32; // vptr
    for (_, fty, _) in fields {
        // Bool is i1 in IR but takes one byte of storage in the class
        // layout; round any sub-byte width up to a full byte.
        let w = field_width(fty);
        bits += w.max(8).next_multiple_of(8);
    }
    let bytes = bits.div_ceil(8);
    let aligned = bytes.next_multiple_of(8);
    aligned - bytes
}

/// Emit setup + postconditions for a pointer parameter using the
/// adversarial model. Dispatches between sized-buffer, struct
/// decomposition, and opaque-fallback paths.
///
/// `cryptol_post_expr`: when `Some`, a write-only (`_Out_`) pointer
/// gets the postcondition `*ptr = {{ <expr> }}` instead of an
/// unconstrained fresh symbolic. Has no effect on non-WriteOnly params.
#[allow(clippy::too_many_arguments)]
pub fn emit_adversarial_param(
    name: &str,
    inner_ty: &TypeInfo,
    behavior: HavocBehavior,
    annotations: &[Annotation],
    setup: &mut String,
    postconds: &mut String,
    cryptol_post_expr: Option<&str>,
) {
    let is_preserved = behavior == HavocBehavior::Preserved;
    let is_write_only = annotations
        .iter()
        .any(|a| matches!(a, Annotation::OutWrites(_)));
    let alloc_kind = if is_preserved {
        "llvm_alloc_readonly"
    } else {
        "llvm_alloc"
    };
    let ann_label = super::havoc::annotation_label(annotations, is_preserved);

    if name == "this" && matches!(inner_ty, TypeInfo::Opaque { size_bytes: 0, .. }) {
        let saw_type = type_to_saw(inner_ty);
        setup.push_str(&format!("\n    // Parameter: {name} ({ann_label})\n"));
        setup.push_str(&format!(
            "    {name}_ptr <- {alloc_kind} ({saw_type});\n"
        ));
        return;
    }

    let buffer_size = annotations.iter().find_map(|a| match a {
        Annotation::InReads(n) if *n > 0 => Some(*n),
        Annotation::OutWrites(n) if *n > 0 => Some(*n),
        _ => None,
    });
    if let Some(n) = buffer_size {
        emit_sized_buffer(
            name, inner_ty, n, is_preserved, alloc_kind, &ann_label, setup, postconds,
        );
        return;
    }

    if let TypeInfo::Struct {
        name: struct_name,
        fields,
        ..
    } = inner_ty
    {
        if !fields.is_empty() {
            emit_struct_decomposed(
                name,
                struct_name,
                fields,
                is_preserved,
                alloc_kind,
                &ann_label,
                setup,
                postconds,
            );
            return;
        }
    }

    emit_opaque_pointer(
        name,
        inner_ty,
        is_preserved,
        is_write_only,
        alloc_kind,
        &ann_label,
        setup,
        postconds,
        cryptol_post_expr,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_opaque_pointer(
    name: &str,
    inner_ty: &TypeInfo,
    is_preserved: bool,
    is_write_only: bool,
    alloc_kind: &str,
    ann_label: &str,
    setup: &mut String,
    postconds: &mut String,
    cryptol_post_expr: Option<&str>,
) {
    let saw_type = type_to_saw(inner_ty);
    setup.push_str(&format!("\n    // Parameter: {name} ({ann_label})\n"));
    setup.push_str(&format!(
        "    {name}_ptr <- {alloc_kind} ({saw_type});\n"
    ));
    if is_preserved {
        setup.push_str(&format!(
            "    {name}_val <- llvm_fresh_var \"{name}\" ({saw_type});\n"
        ));
        setup.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n"
        ));
        postconds.push_str(&format!("    // {name}: {ann_label} → memory unchanged\n"));
        postconds.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n"
        ));
    } else if let (true, Some(expr)) = (is_write_only, cryptol_post_expr) {
        postconds.push_str(&format!(
            "    // {name}: {ann_label} → value defined by Cryptol spec\n"
        ));
        postconds.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {{{{ {expr} }}}});\n"
        ));
    } else {
        postconds.push_str(&format!(
            "    // {name}: {ann_label} → solver chooses ANY final value\n"
        ));
        postconds.push_str(&format!(
            "    {name}_after <- llvm_fresh_var \"{name}_after\" ({saw_type});\n"
        ));
        postconds.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {name}_after);\n"
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_sized_buffer(
    name: &str,
    inner_ty: &TypeInfo,
    n: usize,
    is_preserved: bool,
    alloc_kind: &str,
    ann_label: &str,
    setup: &mut String,
    postconds: &mut String,
) {
    let elem_saw = match inner_ty {
        TypeInfo::Pointer(elem) => type_to_saw(elem),
        _ => type_to_saw(inner_ty),
    };
    let buf_type = format!("llvm_array {n} ({elem_saw})");
    setup.push_str(&format!(
        "\n    // Parameter: {name} ({ann_label}, buffer[{n}])\n"
    ));
    setup.push_str(&format!("    {name}_ptr <- {alloc_kind} ({buf_type});\n"));
    setup.push_str(&format!(
        "    {name}_val <- llvm_fresh_var \"{name}\" ({buf_type});\n"
    ));
    setup.push_str(&format!(
        "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n"
    ));
    if is_preserved {
        postconds.push_str(&format!(
            "    // {name}: _In_reads_({n}) → buffer preserved\n"
        ));
        postconds.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n"
        ));
    } else {
        postconds.push_str(&format!(
            "    // {name}: _Out_writes_({n}) → buffer HAVOCED (any {n} elements)\n"
        ));
        postconds.push_str(&format!(
            "    {name}_after <- llvm_fresh_var \"{name}_after\" ({buf_type});\n"
        ));
        postconds.push_str(&format!(
            "    llvm_points_to {name}_ptr (llvm_term {name}_after);\n"
        ));
    }
}

/// Emit a struct parameter with per-field decomposition.
#[allow(clippy::too_many_arguments)]
pub fn emit_struct_decomposed(
    param_name: &str,
    struct_name: &str,
    fields: &[(String, TypeInfo)],
    is_preserved: bool,
    alloc_kind: &str,
    ann_label: &str,
    setup: &mut String,
    postconds: &mut String,
) {
    let saw_type = type_to_saw(&TypeInfo::Struct {
        name: struct_name.to_string(),
        size_bytes: None,
        fields: fields.to_vec(),
    });

    setup.push_str(&format!(
        "\n    // Parameter: {param_name} → {struct_name} ({ann_label})\n"
    ));
    setup.push_str(&format!(
        "    {param_name}_ptr <- {alloc_kind} ({saw_type});\n"
    ));

    let mut pre_field_terms = Vec::new();
    let mut nested_ptrs: Vec<(String, String, bool)> = Vec::new();

    for (field_name, field_ty) in fields {
        let var_name = format!("{param_name}_{field_name}");
        match field_ty {
            TypeInfo::Pointer(pointee) => {
                let pointee_saw = type_to_saw(pointee);
                let nested_alloc = if is_preserved {
                    "llvm_alloc_readonly"
                } else {
                    "llvm_alloc"
                };
                setup.push_str(&format!(
                    "    {var_name}_ptr <- {nested_alloc} ({pointee_saw});\n"
                ));
                setup.push_str(&format!(
                    "    {var_name}_val <- llvm_fresh_var \"{var_name}\" ({pointee_saw});\n"
                ));
                setup.push_str(&format!(
                    "    llvm_points_to {var_name}_ptr (llvm_term {var_name}_val);\n"
                ));
                pre_field_terms.push(format!("{var_name}_ptr"));
                nested_ptrs.push((var_name, pointee_saw, true));
            }
            _ => {
                let field_saw = type_to_saw(field_ty);
                setup.push_str(&format!(
                    "    {var_name} <- llvm_fresh_var \"{var_name}\" ({field_saw});\n"
                ));
                pre_field_terms.push(format!("llvm_term {var_name}"));
                nested_ptrs.push((var_name, field_saw, false));
            }
        }
    }

    setup.push_str(&format!(
        "    llvm_points_to {param_name}_ptr (llvm_struct_value\n        [ {} ]);\n",
        pre_field_terms.join("\n        , "),
    ));

    if is_preserved {
        postconds.push_str(&format!(
            "    // {param_name} ({struct_name}): {ann_label} → all fields preserved\n"
        ));
        let mut post_terms = Vec::new();
        for (var_name, _, is_ptr) in &nested_ptrs {
            if *is_ptr {
                postconds.push_str(&format!(
                    "    llvm_points_to {var_name}_ptr (llvm_term {var_name}_val);\n"
                ));
                post_terms.push(format!("{var_name}_ptr"));
            } else {
                post_terms.push(format!("llvm_term {var_name}"));
            }
        }
        postconds.push_str(&format!(
            "    llvm_points_to {param_name}_ptr (llvm_struct_value\n        [ {} ]);\n",
            post_terms.join("\n        , "),
        ));
    } else {
        postconds.push_str(&format!(
            "    // {param_name} ({struct_name}): {ann_label} → ALL fields HAVOCED\n"
        ));
        let mut post_terms = Vec::new();
        for (var_name, saw_type, is_ptr) in &nested_ptrs {
            if *is_ptr {
                postconds.push_str(&format!(
                    "    // {var_name}: pointer target → HAVOCED\n"
                ));
                postconds.push_str(&format!(
                    "    {var_name}_after <- llvm_fresh_var \"{var_name}_after\" ({saw_type});\n"
                ));
                postconds.push_str(&format!(
                    "    llvm_points_to {var_name}_ptr (llvm_term {var_name}_after);\n"
                ));
                let ptr_after = format!("{var_name}_ptr_after");
                postconds.push_str(&format!(
                    "    {ptr_after} <- llvm_alloc ({saw_type});\n"
                ));
                post_terms.push(ptr_after);
            } else {
                postconds.push_str(&format!(
                    "    {var_name}_after <- llvm_fresh_var \"{var_name}_after\" ({saw_type});\n"
                ));
                post_terms.push(format!("llvm_term {var_name}_after"));
            }
        }
        postconds.push_str(&format!(
            "    llvm_points_to {param_name}_ptr (llvm_struct_value\n        [ {} ]);\n",
            post_terms.join("\n        , "),
        ));
    }
}
