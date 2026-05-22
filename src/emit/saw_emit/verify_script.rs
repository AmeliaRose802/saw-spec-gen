//! Complete end-to-end verification script generation (`verify.saw`).
//!
//! Coordinates everything else: loads bitcode, wires vtable overrides,
//! imports the Cryptol spec, builds the equivalence body, and invokes
//! `llvm_verify`.

use super::names::{sanitize_name, spec_safe_id, stub_function_name};
use super::overrides::{container_layout_for, emit_container_this};
use super::path_utils::pathdiff_or_absolute;
use super::stubs::AssembledStubs;
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use anyhow::{Context, Result};
use std::collections::HashSet;
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
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let interface_classes: HashSet<String> =
        vmethods.iter().map(|m| m.class_name.clone()).collect();
    let interface_of = make_interface_of(&interface_classes);

    let bitcode_rel = pathdiff_or_absolute(bitcode_path, output_dir);
    let cryptol_rel = pathdiff_or_absolute(cryptol_spec_path, output_dir);

    let needs_operator_new = external_calls.iter().any(is_operator_new);
    let needs_experimental =
        !external_calls.is_empty() || has_interfaces || !sub_callee_specs.is_empty();

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

    // `__attribute__((annotate("..."))` on a parameter (used by the SAL
    // shim under demo/) compiles into a `call @llvm.var.annotation` in
    // every callsite's prelude. SAW can't simulate that intrinsic and
    // bails out with "No implementation or override found for pointer".
    // Inject a no-op assume-spec override whenever the target function
    // carries SAL annotations on any parameter, so the verifier can
    // silently pass through them.
    let needs_var_annotation_ov = target_fn
        .params
        .iter()
        .any(|p| !p.annotations.is_empty());
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

    if !sub_callee_specs.is_empty() {
        step += 1;
        emit_sub_callee_step(&mut out, step, sub_callee_specs, &mut override_names);
    }

    if !has_interfaces {
        step += 1;
        out.push_str(&format!("// Step {step}: Import Cryptol spec\n"));
        out.push_str(&format!("import \"{}\";\n\n", cryptol_rel));
    }

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
    );

    emit_postcondition_and_close(
        &mut out,
        cryptol_fn,
        &cryptol_args,
        &execute_args,
        sub_callee_specs,
    );

    step += 1;
    emit_verify_step(
        &mut out,
        step,
        function_name,
        cryptol_fn,
        mangled_name,
        has_interfaces,
        vmethods,
        constructors,
        override_names,
    );

    let script_path = output_dir.join("verify.saw");
    fs::write(&script_path, &out)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    let _ = all_globals; // silence the lint; consumed indirectly via target_spec
    Ok(())
}

/// Build a closure that maps `TypeInfo` to a known interface class name.
fn make_interface_of(interface_classes: &HashSet<String>) -> impl Fn(&TypeInfo) -> Option<String> + '_ {
    move |ty: &TypeInfo| {
        if let TypeInfo::Pointer(inner) = ty {
            let name_opt = match inner.as_ref() {
                TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.as_str()),
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

fn is_operator_new(spec: &SpecConstraint) -> bool {
    spec.function_name == "operator new"
        || spec.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z")
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

fn emit_load_bitcode_step(
    out: &mut String,
    step: &mut u32,
    has_interfaces: bool,
    bitcode_rel: &str,
    stubs_status: &AssembledStubs,
) {
    out.push_str(&format!("// Step {step}: Load bitcode"));
    if has_interfaces {
        let stubs_filename = stubs_status.script_filename().unwrap_or("vtable_stubs.bc");
        out.push_str(" + vtable stubs\n");
        if let AssembledStubs::TextOnly { ll_filename } = stubs_status {
            out.push_str("//\n");
            out.push_str("// WARNING: vtable_stubs.bc was NOT auto-assembled at gen time\n");
            out.push_str("//          (llvm-as / clang not found on PATH). Assemble it before\n");
            out.push_str("//          running SAW:\n");
            out.push_str(&format!(
                "//            llvm-as {ll_filename} -o vtable_stubs.bc\n"
            ));
            out.push_str(&format!(
                "//          (or)  clang -c -emit-llvm {ll_filename} -o vtable_stubs.bc\n"
            ));
            out.push_str("//          The load line below already references vtable_stubs.bc.\n");
        } else if let AssembledStubs::Bitcode { assembler, .. } = stubs_status {
            out.push_str(&format!(
                "// vtable_stubs.bc was assembled from vtable_stubs.ll via `{assembler}`.\n",
            ));
        }
        out.push_str(&format!("m_main  <- llvm_load_module \"{bitcode_rel}\";\n"));
        out.push_str(&format!(
            "m_stubs <- llvm_load_module \"{stubs_filename}\";\n"
        ));
        out.push_str("m       <- llvm_combine_modules m_main [m_stubs];\n\n");
    } else {
        out.push('\n');
        out.push_str(&format!("m <- llvm_load_module \"{bitcode_rel}\";\n\n"));
    }
    let _ = step; // mutation handled by caller
}

/// Emit a no-op `llvm_unsafe_assume_spec` for `llvm.var.annotation.p0.p0`.
///
/// Clang lowers `__attribute__((annotate("...")))` on a local/parameter
/// into a call to this intrinsic in every annotated function. SAW can't
/// simulate it natively (the symbol has no body and no MIR-level
/// equivalent), so without an override SAW aborts the verification with
/// "No implementation or override found for pointer".
///
/// The override accepts five arbitrary parameters (matching the
/// intrinsic's i8* / i8* / i8* / i32 / i8* signature in LLVM 20) and
/// returns void without touching any state. That's sound because the
/// intrinsic itself has no observable effect at runtime -- it carries
/// purely-static metadata for downstream LLVM passes.
fn emit_var_annotation_override(
    out: &mut String,
    step: u32,
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!(
        "// Step {step}: No-op override for llvm.var.annotation (SAL/__attribute__((annotate)))\n"
    ));
    out.push_str("let var_annotation_spec = do {\n");
    out.push_str("    p1 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    p2 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    p3 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    line <- llvm_fresh_var \"line\" (llvm_int 32);\n");
    out.push_str("    p4 <- llvm_fresh_pointer (llvm_int 8);\n");
    out.push_str("    llvm_execute_func [p1, p2, p3, llvm_term line, p4];\n");
    out.push_str("};\n");
    out.push_str(
        "ov_var_annotation <- llvm_unsafe_assume_spec m \"llvm.var.annotation.p0.p0\" var_annotation_spec;\n\n",
    );
    override_names.push("ov_var_annotation".to_string());
}

fn emit_external_overrides_step(
    out: &mut String,
    step: u32,
    external_calls: &[SpecConstraint],
    needs_operator_new: bool,
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!("// Step {step}: Assume external functions\n"));
    let mut included: HashSet<String> = HashSet::new();
    for spec in external_calls {
        if is_operator_new(spec) {
            continue;
        }
        let safe_name = spec_safe_id(spec);
        if !included.insert(safe_name.clone()) {
            continue;
        }
        out.push_str(&format!(
            "include \"specs_experimental/{safe_name}_auto_spec.saw\";\n",
        ));
        override_names.push(format!("ov_{safe_name}"));
    }
    if needs_operator_new {
        out.push_str("include \"specs_experimental/operator_new_auto_spec.saw\";\n");
        override_names.push("ov_new".to_string());
    }
}

fn emit_sub_callee_step(
    out: &mut String,
    step: u32,
    sub_callee_specs: &[SpecConstraint],
    override_names: &mut Vec<String>,
) {
    out.push_str(&format!(
        "// Step {step}: Sub-function adversarial overrides (compositional)\n",
    ));
    let mut sub_included: HashSet<String> = HashSet::new();
    for spec in sub_callee_specs {
        let safe_name = spec_safe_id(spec);
        if !sub_included.insert(safe_name.clone()) {
            continue;
        }
        out.push_str(&format!(
            "include \"specs_experimental/{safe_name}_auto_spec.saw\";\n",
        ));
        override_names.push(format!("ov_{safe_name}"));
    }
    out.push('\n');
}

#[allow(clippy::too_many_arguments)]
fn emit_equiv_spec_body(
    out: &mut String,
    step: u32,
    function_name: &str,
    cryptol_fn: &str,
    target_spec: &SpecConstraint,
    target_fn: &FunctionInfo,
    interface_classes: &HashSet<String>,
    interface_of: &dyn Fn(&TypeInfo) -> Option<String>,
    _all_globals: &[GlobalVarInfo],
) -> (Vec<String>, Vec<String>) {
    out.push_str(&format!(
        "// Step {step}: Equivalence spec — C++ {function_name} == Cryptol {cryptol_fn}\n",
    ));
    out.push_str(&format!("let {function_name}_equiv_spec = do {{\n"));

    let mut cryptol_args = Vec::new();
    let mut execute_args = Vec::new();
    for (i, param) in target_spec.params.iter().enumerate() {
        let iface = target_fn.params.get(i).and_then(|p| interface_of(&p.ty));
        if let Some(iface_name) = iface {
            let safe_iface = sanitize_name(&iface_name).to_lowercase();
            let ptr_name = format!("{}_ptr", param.name);
            out.push_str(&format!(
                "    // {} : {}* — wire vptr to stub vtable\n",
                param.name, iface_name,
            ));
            out.push_str(&format!(
                "    {ptr_name} <- alloc_{safe_iface}_this;\n"
            ));
            execute_args.push(ptr_name);
            continue;
        }
        if let Some(container) = target_fn
            .params
            .get(i)
            .and_then(|p| container_layout_for(&p.ty, interface_classes))
        {
            emit_container_this(out, &param.name, &container);
            execute_args.push(format!("{}_ptr", param.name));
            continue;
        }
        match param.alloc_type {
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.name, param.saw_type,
                ));
                cryptol_args.push(param.name.clone());
                execute_args.push(format!("llvm_term {}", param.name));
            }
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                let alloc_fn = if param.alloc_type == AllocType::AllocReadonly {
                    "llvm_alloc_readonly"
                } else {
                    "llvm_alloc"
                };
                let ptr_name = format!("{}_ptr", param.name);
                let val_name = param.name.clone();
                out.push_str(&format!(
                    "    {ptr_name} <- {alloc_fn} ({});\n",
                    param.saw_type,
                ));
                out.push_str(&format!(
                    "    {val_name} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {ptr_name} (llvm_term {val_name});\n",
                ));
                cryptol_args.push(val_name);
                execute_args.push(ptr_name);
            }
        }
    }
    out.push('\n');

    for global in &target_spec.referenced_globals {
        out.push_str(&format!(
            "    llvm_alloc_global \"{}\";\n",
            global.mangled_name,
        ));
        if let Some(init) = &global.init_value {
            let bits = match &global.ty {
                TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => *b,
                TypeInfo::Bool => 1,
                _ => 32,
            };
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {{{{ {} : [{}] }}}});\n",
                global.mangled_name, init, bits,
            ));
        }
        out.push('\n');
    }

    (cryptol_args, execute_args)
}

fn emit_postcondition_and_close(
    out: &mut String,
    cryptol_fn: &str,
    cryptol_args: &[String],
    execute_args: &[String],
    sub_callee_specs: &[SpecConstraint],
) {
    out.push_str(&format!(
        "    llvm_execute_func [{}];\n\n",
        execute_args.join(", "),
    ));
    let cryptol_call = if cryptol_args.is_empty() {
        cryptol_fn.to_string()
    } else {
        format!("{} {}", cryptol_fn, cryptol_args.join(" "))
    };
    if sub_callee_specs.is_empty() {
        out.push_str("    // Postcondition: C++ result == Cryptol spec\n");
        out.push_str(&format!(
            "    llvm_return (llvm_term {{{{ {} }}}});\n",
            cryptol_call,
        ));
    } else {
        out.push_str("    // TODO: Compositional postcondition — fill in by hand.\n");
        out.push_str("    //\n");
        out.push_str("    // The sub-function overrides above return fresh symbolic values.\n");
        out.push_str("    // To check functional correctness, capture those returns (e.g.\n");
        out.push_str("    //   ret_helper <- llvm_fresh_var \"ret_helper\" (llvm_int 32);\n");
        out.push_str("    // inside the matching override spec) and thread them into the\n");
        out.push_str("    // Cryptol equivalence call below.  The auto-derived call only uses\n");
        out.push_str("    // the target's own parameters and is almost certainly incomplete.\n");
        out.push_str(&format!(
            "    llvm_return (llvm_term {{{{ {} }}}});  // <-- replace with full mapping\n",
            cryptol_call,
        ));
    }
    out.push_str("};\n\n");
}

#[allow(clippy::too_many_arguments)]
fn emit_verify_step(
    out: &mut String,
    step: u32,
    function_name: &str,
    cryptol_fn: &str,
    mangled_name: &str,
    has_interfaces: bool,
    vmethods: &[InterfaceMethod],
    constructors: &[ClassConstructor],
    override_names: Vec<String>,
) {
    out.push_str(&format!("// Step {step}: Verify equivalence\n"));
    out.push_str(&format!(
        "print \"=== Checking: {function_name} == {cryptol_fn} (Cryptol) ===\";\n",
    ));

    let mut overrides = override_names;
    if has_interfaces {
        let ctor_classes: HashSet<&str> = constructors
            .iter()
            .map(|c| c.class_name.as_str())
            .collect();
        for class in &ctor_classes {
            let safe_class = sanitize_name(class).to_lowercase();
            overrides.push(format!("ov_{safe_class}_ctor"));
        }
        for method in vmethods {
            if method.is_override {
                continue;
            }
            let stub_name = stub_function_name(method);
            let safe_name = sanitize_name(&stub_name);
            overrides.push(format!("ov_{safe_name}"));
        }
    }

    let overrides_str = if overrides.len() <= 4 {
        format!("[{}]", overrides.join(", "))
    } else {
        let mut s = String::from("[\n");
        for (i, ov) in overrides.iter().enumerate() {
            if i == 0 {
                s.push_str(&format!("     {ov}"));
            } else {
                s.push_str(&format!(",\n     {ov}"));
            }
        }
        s.push(']');
        s
    };

    out.push_str(&format!(
        "llvm_verify m \"{mangled_name}\"\n    {overrides_str}\n    false {function_name}_equiv_spec z3;\n\n",
    ));
    out.push_str(&format!(
        "print \"=== VERIFIED: {function_name} == {cryptol_fn} ===\";\n",
    ));
}
