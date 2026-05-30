//! Adversarial havoc spec generation for virtual / external methods.
//!
//! For a method whose body we don't model, the spec lets the solver
//! pick any post-call state consistent with annotations and types.
//! Memory marked const / `_In_*` is preserved across the call;
//! everything else is havoced.
//!
//! Per-parameter setup/postcondition emitters live in
//! [`super::havoc_params`].

use super::havoc_params::{emit_adversarial_param, emit_this_full_class_havoc};
use super::names::{sanitize_name, stub_function_name};
use super::types::sret_inner_ir_type;
use super::writer::is_void_saw_type;
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;

/// Whether a parameter's memory is preserved or havoced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HavocBehavior {
    /// Memory preserved after call (const / `_In_`).
    Preserved,
    /// Memory havoced: solver picks any final value
    /// (`_Out_` / `_Inout_` / mutable).
    Havoced,
}

/// Resolve parameter behavior using the **strictest constraint wins** rule.
///
/// - Type system `const T*` / `const T&`     → Preserved
/// - SAL `_In_` / `_In_reads_`               → Preserved
/// - SAL `_Out_` / `_Out_writes_`            → Havoced (unless const overrides)
/// - SAL `_Inout_`                           → Havoced (unless const overrides)
/// - non-const with no annotation             → Havoced
pub fn resolve_param_behavior(param: &ParamInfo) -> HavocBehavior {
    let type_says_const = param.mutability == Mutability::Readonly;
    let sal_says_readonly = param
        .annotations
        .iter()
        .any(|a| matches!(a, Annotation::InReads(_)));
    if type_says_const || sal_says_readonly {
        return HavocBehavior::Preserved;
    }
    let sal_says_writable = param
        .annotations
        .iter()
        .any(|a| matches!(a, Annotation::OutWrites(_) | Annotation::Inout));
    if sal_says_writable {
        return HavocBehavior::Havoced;
    }
    match param.mutability {
        Mutability::Readonly => HavocBehavior::Preserved,
        Mutability::Mutable | Mutability::WriteOnly => HavocBehavior::Havoced,
    }
}

/// Generate an adversarial havoc SAW spec for one virtual method.
///
/// **Annotation semantics** (SAL / prefast / const):
///   - `_In_` / `const T*` / `_In_reads_(n)` → preserved (read-only)
///   - `_Out_` / `_Out_writes_(n)` → havoced (write-only, any final value)
///   - `_Inout_` / `T*` (non-const) → havoced (read-write, any final value)
///   - `const` method → `this` is preserved
///   - non-const method → `this` is havoced (all fields get arbitrary values)
///
/// **NOT modeled** (noted in the output as a TODO):
///   - Pointer chains deeper than 2 levels.
///   - Aliased memory not visible in the type signature.
pub fn generate_havoc_spec(
    imethod: &InterfaceMethod,
    globals: &[GlobalVarInfo],
    layout: Option<&ClassConstructor>,
    cryptol_fn: Option<&str>,
) -> String {
    let mut out = String::new();
    let func = &imethod.method;
    let stub_name = stub_function_name(imethod);
    let safe_name = sanitize_name(&stub_name);

    emit_header(&mut out, imethod);
    out.push_str(&format!("let {safe_name}_havoc : LLVMSetup () = do {{\n"));

    let cryptol_post_expr = cryptol_post_expr(func, cryptol_fn);
    let (setup, postconds, args, sret_saw_type) =
        build_param_blocks(func, layout, cryptol_post_expr.as_deref());

    out.push_str(&setup);
    out.push_str(&format!("\n    llvm_execute_func [{}];\n", args.join(", "),));

    emit_return(&mut out, &func.return_type, sret_saw_type.as_deref());

    if !postconds.is_empty() {
        out.push_str("\n    // --- Postconditions ---\n");
        out.push_str(&postconds);
    }
    if !globals.is_empty() {
        emit_global_havoc(&mut out, globals);
    }

    out.push_str("};\n");
    out
}

fn emit_header(out: &mut String, imethod: &InterfaceMethod) {
    out.push_str(&format!(
        "// Adversarial havoc spec for {}::{}\n",
        imethod.class_name, imethod.method.name,
    ));
    out.push_str("//\n");
    out.push_str("// Models ANY implementation consistent with type-system and SAL constraints.\n");
    out.push_str(
        "// The solver can choose ANY value for mutable/writable memory after the call.\n",
    );
    out.push_str("// Const/_In_ memory is preserved — the solver cannot modify it.\n");
    out.push_str("//\n");
    out.push_str("// Annotation key:\n");
    out.push_str("//   _In_ / const    → PRESERVED (read-only, unchanged after call)\n");
    out.push_str("//   _Out_           → HAVOCED   (write-only, any final value)\n");
    out.push_str("//   _Inout_ / &mut  → HAVOCED   (read-write, any final value)\n");
    out.push_str("//   no annotation   → inferred from const/mutable\n");
    out.push_str("//\n");
    out.push_str("// NOT modeled: pointer chains >2 deep, aliased memory.\n\n");
}

/// Build the Cryptol-call expression used inside `_Out_` postconditions
/// (when a user-supplied Cryptol function name is provided). Inputs are
/// the function's scalar parameters and any preserved-pointer parameters
/// (their `*_val` symbolic value).
fn cryptol_post_expr(func: &FunctionInfo, cryptol_fn: Option<&str>) -> Option<String> {
    cryptol_fn.map(|fn_name| {
        let mut input_args: Vec<String> = Vec::new();
        for p in &func.params {
            if p.name == "this" {
                continue;
            }
            match &p.ty {
                TypeInfo::Pointer(_) => {
                    if resolve_param_behavior(p) == HavocBehavior::Preserved {
                        input_args.push(format!("{}_val", p.name));
                    }
                }
                _ => {
                    input_args.push(p.name.clone());
                }
            }
        }
        if input_args.is_empty() {
            fn_name.to_string()
        } else {
            format!("{fn_name} {}", input_args.join(" "))
        }
    })
}

/// Build the per-parameter setup + postcondition strings and the
/// argument list for `llvm_execute_func`. Also splices in an sret
/// buffer at position 1 when the return type lowers to a hidden
/// pointer under the MSVC ABI.
fn build_param_blocks(
    func: &FunctionInfo,
    layout: Option<&ClassConstructor>,
    cryptol_post_expr: Option<&str>,
) -> (String, String, Vec<String>, Option<String>) {
    let mut setup = String::new();
    let mut postconds = String::new();
    let mut args = Vec::new();

    for param in &func.params {
        let is_indirect = matches!(&param.ty, TypeInfo::Pointer(_));
        if !is_indirect {
            let saw_type = type_to_saw(&param.ty);
            setup.push_str(&format!(
                "\n    // Parameter: {} (pass-by-value)\n",
                param.name,
            ));
            setup.push_str(&format!(
                "    {} <- llvm_fresh_var \"{}\" ({});\n",
                param.name, param.name, saw_type,
            ));
            args.push(format!("llvm_term {}", param.name));
            continue;
        }
        let inner_ty = match &param.ty {
            TypeInfo::Pointer(inner) => inner.as_ref(),
            _ => unreachable!(),
        };
        let behavior = resolve_param_behavior(param);
        if param.name == "this"
            && behavior == HavocBehavior::Havoced
            && layout.map(|c| !c.layout_fields.is_empty()).unwrap_or(false)
        {
            emit_this_full_class_havoc(layout.unwrap(), &mut setup, &mut postconds);
            args.push("this_ptr".to_string());
            continue;
        }
        emit_adversarial_param(
            &param.name,
            inner_ty,
            behavior,
            &param.annotations,
            &mut setup,
            &mut postconds,
            cryptol_post_expr,
        );
        args.push(format!("{}_ptr", param.name));
    }

    let sret_saw_type: Option<String> =
        sret_inner_ir_type(&func.return_type).map(|_| match &func.return_type {
            TypeInfo::Opaque { size_bytes: 0, .. } => "llvm_array 16 (llvm_int 8)".to_string(),
            TypeInfo::Struct {
                size_bytes: None, ..
            } => "llvm_array 16 (llvm_int 8)".to_string(),
            other => type_to_saw(other),
        });
    if let Some(saw_type) = &sret_saw_type {
        setup.push_str("\n    // sret: aggregate return passed via hidden output pointer\n");
        setup.push_str("    // (MSVC ABI: parameter index 1, immediately after `this`).\n");
        setup.push_str(&format!("    result_ptr <- llvm_alloc ({saw_type});\n"));
        if args.is_empty() {
            args.push("result_ptr".to_string());
        } else {
            args.insert(1, "result_ptr".to_string());
        }
    }

    (setup, postconds, args, sret_saw_type)
}

fn emit_return(out: &mut String, return_type: &TypeInfo, sret_saw_type: Option<&str>) {
    if let Some(saw_type) = sret_saw_type {
        out.push_str("\n    // sret return: solver chooses any final value for *result_ptr\n");
        out.push_str(&format!(
            "    ret <- llvm_fresh_var \"ret\" ({saw_type});\n"
        ));
        out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
    } else {
        let ret_saw = type_to_saw(return_type);
        if !is_void_saw_type(&ret_saw) {
            out.push_str("\n    // Return: unconstrained (solver chooses any value)\n");
            out.push_str(&format!("    ret <- llvm_fresh_var \"ret\" ({ret_saw});\n"));
            out.push_str("    llvm_return (llvm_term ret);\n");
        }
    }
}

fn emit_global_havoc(out: &mut String, globals: &[GlobalVarInfo]) {
    out.push_str("\n    // --- Havoced globals ---\n");
    out.push_str("    // A virtual method could be ANY implementation, including one\n");
    out.push_str("    // that writes to global variables. Model as unconstrained.\n");
    for global in globals {
        let safe = sanitize_name(&global.name);
        let saw_type = type_to_saw(&global.ty);
        out.push_str(&format!(
            "    {safe}_post <- llvm_fresh_var \"{safe}_post\" ({saw_type});\n",
        ));
        out.push_str(&format!(
            "    llvm_points_to (llvm_global \"{}\") (llvm_term {safe}_post);\n",
            global.mangled_name,
        ));
    }
}

/// Human-readable label for the annotation-derived behavior, used in
/// inline comments alongside each parameter.
pub fn annotation_label(annotations: &[Annotation], is_preserved: bool) -> String {
    for ann in annotations {
        match ann {
            Annotation::InReads(0) => return "_In_ → preserved".into(),
            Annotation::InReads(n) => return format!("_In_reads_({n}) → preserved"),
            Annotation::OutWrites(0) => return "_Out_ → HAVOCED".into(),
            Annotation::OutWrites(n) => return format!("_Out_writes_({n}) → HAVOCED"),
            Annotation::Inout => return "_Inout_ → HAVOCED".into(),
            _ => {}
        }
    }
    if is_preserved {
        "const → preserved".into()
    } else {
        "mutable → HAVOCED".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clang_ast::InterfaceMethod;
    use crate::constraints::{FunctionInfo, Mutability, Nullability, ParamInfo};

    fn make_iface_method(class: &str, name: &str, ret: TypeInfo, offset: u64) -> InterfaceMethod {
        InterfaceMethod {
            class_name: class.into(),
            method: FunctionInfo {
                name: name.into(),
                mangled_name: None,
                params: vec![ParamInfo {
                    name: "this".into(),
                    ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                        name: "Self".into(),
                        size_bytes: 0,
                    })),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                }],
                return_type: ret,
                can_throw: false,
                is_virtual: true,
                has_body: false,
                is_system: false,
                annotations: vec![],
                referenced_globals: vec![],
                called_functions: vec![],
            },
            is_pure: true,
            is_override: false,
            source_offset: offset,
        }
    }

    #[test]
    fn test_havoc_spec_uses_points_to_for_sret_return() {
        let tuple_ret = TypeInfo::Opaque {
            name: "std::tuple<A,B>".into(),
            size_bytes: 0,
        };
        let method = make_iface_method("IKeyStore", "Read", tuple_ret, 100);
        let spec = generate_havoc_spec(&method, &[], None, None);
        assert!(spec.contains("result_ptr <- llvm_alloc"));
        assert!(spec.contains("llvm_points_to result_ptr"));
        assert!(!spec.contains("llvm_return"));
        let exec_line = spec
            .lines()
            .find(|l| l.contains("llvm_execute_func"))
            .expect("execute_func missing");
        let this_pos = exec_line.find("this_ptr").expect("this_ptr missing");
        let result_pos = exec_line.find("result_ptr").expect("result_ptr missing");
        assert!(this_pos < result_pos);
    }

    #[test]
    fn resolve_behavior_const_pointer_is_preserved() {
        let p = ParamInfo {
            name: "x".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
            mutability: Mutability::Readonly,
            nullable: Nullability::NonNull,
            annotations: vec![],
        };
        assert_eq!(resolve_param_behavior(&p), HavocBehavior::Preserved);
    }

    #[test]
    fn resolve_behavior_in_annotation_wins_over_mutable_type() {
        let p = ParamInfo {
            name: "x".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: vec![Annotation::InReads(0)],
        };
        assert_eq!(resolve_param_behavior(&p), HavocBehavior::Preserved);
    }

    #[test]
    fn resolve_behavior_inout_forces_havoc() {
        let p = ParamInfo {
            name: "x".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
            mutability: Mutability::Mutable,
            nullable: Nullability::NonNull,
            annotations: vec![Annotation::Inout],
        };
        assert_eq!(resolve_param_behavior(&p), HavocBehavior::Havoced);
    }

    #[test]
    fn void_pointer_param_does_not_emit_comment_in_alloc_slot() {
        // Bug #11 regression guard at the havoc-spec emission layer:
        // an opaque mutable `void*` parameter must never produce
        // `llvm_alloc (// void)` (which SAW parses as an unterminated
        // expression). Any `//` substring inside parentheses indicates
        // a code-gen bug.
        let mut method = make_iface_method("MemoryResource", "do_deallocate", TypeInfo::Void, 200);
        // Replace the synthetic `this` with a real `void*` param.
        method.method.params = vec![
            ParamInfo {
                name: "this".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "Self".into(),
                    size_bytes: 0,
                })),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
            ParamInfo {
                name: "p".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::Void)),
                mutability: Mutability::Mutable,
                nullable: Nullability::NonNull,
                annotations: vec![],
            },
        ];
        let spec = generate_havoc_spec(&method, &[], None, None);
        for (i, line) in spec.lines().enumerate() {
            if let Some(open) = line.find('(') {
                let inside = &line[open + 1..];
                assert!(
                    !inside.contains("//"),
                    "line {i} has `//` inside expression: {line}",
                );
            }
        }
        assert!(
            spec.contains("p_ptr <- llvm_alloc (llvm_int 8)"),
            "expected `p_ptr <- llvm_alloc (llvm_int 8)` in:\n{spec}",
        );
    }

    #[test]
    fn annotation_label_recognizes_sal() {
        assert_eq!(
            annotation_label(&[Annotation::InReads(0)], true),
            "_In_ → preserved"
        );
        assert_eq!(
            annotation_label(&[Annotation::InReads(16)], true),
            "_In_reads_(16) → preserved"
        );
        assert_eq!(
            annotation_label(&[Annotation::OutWrites(8)], false),
            "_Out_writes_(8) → HAVOCED"
        );
        assert_eq!(
            annotation_label(&[Annotation::Inout], false),
            "_Inout_ → HAVOCED"
        );
        assert_eq!(annotation_label(&[], true), "const → preserved");
        assert_eq!(annotation_label(&[], false), "mutable → HAVOCED");
    }
}
