//! Complete end-to-end verification script generation (`verify.saw`).
//!
//! Coordinates everything else: loads bitcode, wires vtable overrides,
//! imports the Cryptol spec, builds the equivalence body, and invokes
//! `llvm_verify`.

use super::bitcode_overrides::EmittedBitcodeOverrides;
use super::path_utils::pathdiff_or_absolute;
use super::stubs::AssembledStubs;
use super::verify_script_steps::{
    emit_equiv_spec_body, emit_external_overrides_step, emit_load_bitcode_step,
    emit_postcondition_and_close, emit_sub_callee_step, emit_var_annotation_override,
    emit_verify_step, is_operator_new, InterfaceCtx, PostconditionCtx,
};
use crate::buffer_overrides::BufferOverrides;
use crate::clang_ast::{self, ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use crate::cryptol_sig;
use crate::llvm_ir::IrStructDef;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Emit a complete, runnable SAW verification script that checks a C++
/// function against a Cryptol spec.
#[allow(clippy::too_many_arguments)]
pub fn emit_verification_script(
    bitcode_path: &Path,
    cryptol_spec_path: &Path,
    cryptol_fn: &str,
    function_name: &str,
    mangled_name: &str,
    target_spec: &SpecConstraint,
    target_fn: &FunctionInfo,
    vmethods: &[InterfaceMethod],
    has_interfaces: bool,
    external_calls: &[SpecConstraint],
    sub_callee_specs: &[SpecConstraint],
    all_globals: &[GlobalVarInfo],
    constructors: &[ClassConstructor],
    stubs_status: &AssembledStubs,
    bitcode_overrides: &EmittedBitcodeOverrides,
    ir_struct_defs: &HashMap<String, IrStructDef>,
    output_dir: &Path,
    buffer_overrides: &BufferOverrides,
    has_source_sal_annotations: bool,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let interface_classes: HashSet<String> =
        vmethods.iter().map(|m| m.class_name.clone()).collect();
    let interface_of = make_interface_of(&interface_classes);

    let bitcode_rel = pathdiff_or_absolute(bitcode_path, output_dir);
    let cryptol_rel = pathdiff_or_absolute(cryptol_spec_path, output_dir);

    let needs_operator_new = external_calls.iter().any(is_operator_new);
    let needs_experimental = !external_calls.is_empty()
        || has_interfaces
        || !sub_callee_specs.is_empty()
        || !bitcode_overrides.is_empty();

    let mut out = String::new();
    let mut step = 1u32;

    emit_header(
        &mut out,
        function_name,
        mangled_name,
        &cryptol_rel,
        cryptol_fn,
        needs_experimental,
    );

    emit_load_bitcode_step(
        &mut out,
        &mut step,
        has_interfaces,
        &bitcode_rel,
        stubs_status,
    );

    if has_interfaces {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Vtable havoc overrides + constructor overrides\n"
        ));
        out.push_str(&format!("import \"{}\";\n", cryptol_rel));
        out.push_str("include \"interface_overrides.saw\";\n\n");
    }

    let mut override_names: Vec<String> = Vec::new();

    // Inject a no-op override for `@llvm.var.annotation` intrinsics generated
    // by SAL annotations on parameters (SAW can't simulate them). Only emit
    // when the source actually had SAL annotations — synthetic annotations
    // added by gen-verify passes (length-binding, struct-shape) do NOT
    // produce this intrinsic in the bitcode.
    let needs_var_annotation_ov = has_source_sal_annotations;
    if needs_var_annotation_ov {
        step += 1;
        emit_var_annotation_override(&mut out, step, &mut override_names);
    }

    if !external_calls.is_empty() || needs_operator_new {
        step += 1;
        emit_external_overrides_step(
            &mut out,
            step,
            external_calls,
            needs_operator_new,
            &mut override_names,
        );
    }

    // Bitcode-derived overrides for symbols the AST path filter dropped
    // (notably `printf` and other system-header inline-defined functions)
    // and for variadic functions whose body uses `llvm.va_*` intrinsics
    // SAW cannot symbolically execute. See
    // `src/transform/extern_override_scan.rs` for the analysis and
    // `src/emit/saw_emit/bitcode_overrides.rs` for the emission contract.
    if !bitcode_overrides.is_empty() {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Bitcode-derived extern overrides\n",
        ));
        out.push_str(&bitcode_overrides.snippet);
        out.push('\n');
        override_names.extend(bitcode_overrides.override_names.iter().cloned());
    }

    if !sub_callee_specs.is_empty() {
        step += 1;
        emit_sub_callee_step(&mut out, step, sub_callee_specs, &mut override_names);
    }

    if !has_interfaces {
        step += 1;
        out.push_str(&format!("// Step {step}: Import Cryptol spec\n"));
        out.push_str(&format!("import \"{}\";\n\n", cryptol_rel));
    }

    // Detect sret prestate threading: if the Cryptol function has one
    // extra trailing [N][8] parameter compared to the C++ user-arg
    // count, the model expects the sret buffer's pre-call bytes.
    let sret_prestate = if target_spec.return_constraint.is_sret {
        detect_sret_prestate(
            cryptol_spec_path,
            cryptol_fn,
            target_spec,
            target_fn,
            ir_struct_defs,
        )
    } else {
        None
    };

    step += 1;
    let (cryptol_args, execute_args) = emit_equiv_spec_body(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        target_spec,
        target_fn,
        &interface_classes,
        &interface_of,
        all_globals,
        sret_prestate.as_ref(),
        buffer_overrides,
    );

    let post_ctx = PostconditionCtx {
        sub_callee_specs,
        return_type: &target_fn.return_type,
        is_sret: target_spec.return_constraint.is_sret,
        return_bridge: None,
    };
    emit_postcondition_and_close(
        &mut out,
        cryptol_fn,
        &cryptol_args,
        &execute_args,
        &post_ctx,
        buffer_overrides,
    );

    let iface_ctx = InterfaceCtx {
        has_interfaces,
        vmethods,
        constructors,
    };
    step += 1;
    emit_verify_step(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        mangled_name,
        &iface_ctx,
        override_names,
    );

    let script_path = output_dir.join("verify.saw");
    fs::write(&script_path, &out)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    Ok(())
}

/// Build a closure that maps `TypeInfo` to a known interface class name.
fn make_interface_of(
    interface_classes: &HashSet<String>,
) -> impl Fn(&TypeInfo) -> Option<String> + '_ {
    move |ty: &TypeInfo| {
        if let TypeInfo::Pointer(inner) = ty {
            let name_opt = match inner.as_ref() {
                TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => {
                    Some(name.as_str())
                }
                _ => None,
            };
            if let Some(name) = name_opt {
                if interface_classes.contains(name) {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

fn emit_header(
    out: &mut String,
    function_name: &str,
    mangled_name: &str,
    cryptol_rel: &str,
    cryptol_fn: &str,
    needs_experimental: bool,
) {
    out.push_str(&format!(
        "// Auto-generated SAW verification script\n\
         // Function: {function_name} ({mangled_name})\n\
         // Cryptol spec: {cryptol_rel} :: {cryptol_fn}\n\
         // Generated by saw-spec-gen gen-verify\n\n",
    ));
    if needs_experimental {
        out.push_str("enable_experimental;\n\n");
    }
}

/// How to pass sret pre-state bytes to the Cryptol model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SretPrestate {
    /// Total size of the sret buffer in bytes.
    pub buf_size: usize,
    /// Number of bytes to `take` from the full sret buffer.
    pub take_bytes: usize,
    /// Number of bytes to `drop` before taking.
    pub drop_bytes: usize,
}

/// Detect sret prestate threading by comparing the Cryptol function's
/// arity to the C++ user-arg count. Returns `Some(SretPrestate)` when
/// the Cryptol function has exactly one extra trailing `[K][8]`
/// parameter (the pre-call sret buffer bytes). When `K` equals the
/// full buffer size, `drop_bytes` is 0. When `K` is smaller, walks
/// the return struct's field layout to find the matching field offset.
fn detect_sret_prestate(
    cryptol_spec_path: &Path,
    cryptol_fn: &str,
    target_spec: &SpecConstraint,
    target_fn: &FunctionInfo,
    ir_struct_defs: &HashMap<String, IrStructDef>,
) -> Option<SretPrestate> {
    let sig = cryptol_sig::parse_signature(cryptol_spec_path, cryptol_fn)?;
    let cpp_user_arg_count = target_spec.params.len();

    let cry_k = match cryptol_sig::detect_prestate(&sig, cpp_user_arg_count) {
        Ok(Some(n)) => {
            eprintln!(
                "  sret prestate threading: Cryptol `{cryptol_fn}` has trailing \
                 [{}][8] parameter for pre-call buffer bytes",
                n,
            );
            n
        }
        Ok(None) => return None,
        Err(e) => {
            let has_this = target_fn
                .params
                .first()
                .map(|p| p.name == "this")
                .unwrap_or(false);
            let label = if has_this {
                "instance method"
            } else {
                "free function"
            };
            eprintln!(
                "warning: sret arity mismatch for {label} `{}`:\n  {e}\n  \
                 Workaround: hand-write verify.saw or adjust the Cryptol spec.",
                target_fn.name,
            );
            return None;
        }
    };

    // Derive a name to use for IR struct lookup. Try the SAW alias name
    // first (`llvm_alias "struct.X"`), then fall back to the C++ AST
    // struct name prefixed with `struct.`.
    let ir_lookup_name: Option<String> =
        extract_alias_name(&target_spec.return_constraint.saw_type).or_else(|| {
            match &target_fn.return_type {
                TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => {
                    Some(format!("struct.{name}"))
                }
                _ => None,
            }
        });

    // Get full buffer size from the return type. When the clang AST
    // only has a forward declaration (Opaque with size=0), fall back to
    // the LLVM IR struct definition table.
    let buf_size = match &target_fn.return_type {
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => *n,
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 0 => *size_bytes,
        _ => {
            // Try IR struct defs
            if let Some(ref name) = ir_lookup_name {
                let def = lookup_ir_def(name, ir_struct_defs);
                if let Some(def) = def {
                    use crate::llvm_ir::struct_types::compute_struct_size;
                    let raw_map = build_raw_ir_map(ir_struct_defs);
                    compute_struct_size(def, &raw_map).unwrap_or(0)
                } else {
                    // Try parsing from `llvm_array N (llvm_int 8)`
                    extract_array_byte_count(&target_spec.return_constraint.saw_type).unwrap_or(0)
                }
            } else {
                extract_array_byte_count(&target_spec.return_constraint.saw_type).unwrap_or(0)
            }
        }
    };

    if buf_size == 0 {
        return Some(SretPrestate {
            buf_size: 0,
            take_bytes: cry_k,
            drop_bytes: 0,
        });
    }

    // Full buffer — no slicing needed.
    if cry_k >= buf_size {
        return Some(SretPrestate {
            buf_size,
            take_bytes: cry_k,
            drop_bytes: 0,
        });
    }

    // Walk fields to find the unique field of size K.
    // First try C++ AST fields, then fall back to IR struct fields.
    let matches = match &target_fn.return_type {
        TypeInfo::Struct { fields, .. } => find_field_by_cpp_walk(fields, cry_k, buf_size),
        _ => Vec::new(),
    };
    let matches = if matches.is_empty() {
        // Fall back to IR struct definition field layout.
        ir_lookup_name
            .as_deref()
            .and_then(|name| find_field_by_ir_walk(name, cry_k, ir_struct_defs))
            .unwrap_or_default()
    } else {
        matches
    };

    match matches.len() {
        1 => {
            eprintln!("  → field at offset {} matches [{}][8]", matches[0], cry_k,);
            Some(SretPrestate {
                buf_size,
                take_bytes: cry_k,
                drop_bytes: matches[0],
            })
        }
        0 => {
            eprintln!(
                "warning: Cryptol pre-state expects [{cry_k}][8] but no field \
                 of that size in return type; passing full buffer",
            );
            Some(SretPrestate {
                buf_size,
                take_bytes: cry_k,
                drop_bytes: 0,
            })
        }
        _ => {
            eprintln!(
                "warning: Cryptol pre-state expects [{cry_k}][8] — multiple \
                 fields match at offsets: {}; passing full buffer",
                matches
                    .iter()
                    .map(|o| format!("{o}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            Some(SretPrestate {
                buf_size,
                take_bytes: cry_k,
                drop_bytes: 0,
            })
        }
    }
}

/// Extract the struct name from a SAW type like `llvm_alias "struct.Foo"`.
fn extract_alias_name(saw_type: &str) -> Option<String> {
    let rest = saw_type.strip_prefix("llvm_alias \"")?;
    let name = rest.strip_suffix('"')?;
    Some(name.to_string())
}

/// Extract the byte count from `llvm_array N (llvm_int 8)`.
fn extract_array_byte_count(saw_type: &str) -> Option<usize> {
    let rest = saw_type.strip_prefix("llvm_array ")?;
    let (n_str, suffix) = rest.split_once(' ')?;
    if suffix.trim() == "(llvm_int 8)" {
        n_str.parse().ok()
    } else {
        None
    }
}

/// Walk C++ AST struct fields to find fields matching `target_size`.
fn find_field_by_cpp_walk(
    fields: &[(String, TypeInfo)],
    target_size: usize,
    buf_size: usize,
) -> Vec<usize> {
    let mut matches = Vec::new();
    let mut offset = 0usize;
    let mut walk_valid = true;
    for (fname, fty) in fields {
        if let Some((sz, align)) = clang_ast::cpp_type_size_align(fty) {
            let rem = offset % align;
            if rem != 0 {
                offset += align - rem;
            }
            if sz == target_size {
                matches.push(offset);
            }
            offset += sz;
        } else {
            // Unknown field size — try to infer from remaining space.
            let tail_known: usize = fields
                .iter()
                .skip_while(|(n, _)| n != fname)
                .skip(1)
                .filter_map(|(_, t)| clang_ast::cpp_type_size_align(t).map(|(s, _)| s))
                .sum();
            let inferred = buf_size.saturating_sub(offset).saturating_sub(tail_known);
            if inferred > 0 {
                if inferred == target_size {
                    matches.push(offset);
                }
                offset += inferred;
            } else {
                walk_valid = false;
                break;
            }
        }
    }
    if !walk_valid {
        matches.clear();
    }
    matches
}

/// Walk IR struct fields to find fields matching `target_size` bytes.
fn find_field_by_ir_walk(
    struct_name: &str,
    target_size: usize,
    ir_defs: &HashMap<String, IrStructDef>,
) -> Option<Vec<usize>> {
    // Look up by exact name first, then fall back to suffix match
    // (e.g. "struct.EnrollmentStatus" matches "struct.sdep::EnrollmentStatus").
    let def = lookup_ir_def(struct_name, ir_defs)?;
    // Build a raw-keyed map for size/offset computation.
    let raw_map = build_raw_ir_map(ir_defs);
    let offsets = crate::llvm_ir::struct_types::field_offsets(def, &raw_map)?;
    let mut matches = Vec::new();
    for (i, field_ty) in def.fields.iter().enumerate() {
        let sz = crate::llvm_ir::struct_types::compute_ir_type_size(field_ty, &raw_map)?;
        if sz == target_size {
            matches.push(offsets[i]);
        }
    }
    Some(matches)
}

/// Build a raw-keyed (`%"name"` and `%name`) map from cleaned IR defs,
/// needed by `compute_struct_size` / `compute_ir_type_size`.
fn build_raw_ir_map(ir_defs: &HashMap<String, IrStructDef>) -> HashMap<String, IrStructDef> {
    ir_defs
        .iter()
        .flat_map(|(k, v)| {
            vec![
                (format!("%\"{k}\""), v.clone()),
                (format!("%{k}"), v.clone()),
            ]
        })
        .collect()
}

/// Look up a struct definition by name, falling back to suffix match.
/// Handles namespace differences: `struct.Foo` matches `struct.ns::Foo`.
fn lookup_ir_def<'a>(
    name: &str,
    ir_defs: &'a HashMap<String, IrStructDef>,
) -> Option<&'a IrStructDef> {
    ir_defs.get(name).or_else(|| {
        // Strip the common LLVM prefix (struct./class./union.) and match
        // on the bare C++ name. E.g. "struct.EnrollmentStatus" → look for
        // any key ending in "::EnrollmentStatus".
        let bare = name
            .strip_prefix("struct.")
            .or_else(|| name.strip_prefix("class."))
            .or_else(|| name.strip_prefix("union."))
            .unwrap_or(name);
        let suffix = format!("::{bare}");
        ir_defs
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, v)| v)
    })
}
