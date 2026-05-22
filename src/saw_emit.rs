//! Emit SAW verification scripts from derived constraints.
//!
//! Generates .saw files that can be loaded by SAW to create override
//! specs for unspecified functions. Supports both LLVM and MIR verify modes.

use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Whether to emit LLVM or MIR specs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmitMode {
    Llvm,
    Mir,
}

/// Emit SAW spec files for all constraints.
pub fn emit_saw_specs(
    specs: &[SpecConstraint],
    output_dir: &Path,
    experimental: bool,
) -> Result<()> {
    // Collect globals from the specs themselves (fallback when not passed externally)
    let all_globals: Vec<GlobalVarInfo> = if experimental {
        let mut gs: Vec<_> = specs
            .iter()
            .flat_map(|s| s.referenced_globals.iter().cloned())
            .collect();
        gs.sort_by(|a, b| a.mangled_name.cmp(&b.mangled_name));
        gs.dedup_by(|a, b| a.mangled_name == b.mangled_name);
        gs
    } else {
        vec![]
    };
    emit_saw_specs_with_mode(specs, output_dir, EmitMode::Llvm, experimental, &all_globals)
}

/// Emit SAW spec files with an explicit list of all globals from the translation unit.
pub fn emit_saw_specs_with_globals(
    specs: &[SpecConstraint],
    output_dir: &Path,
    experimental: bool,
    all_globals: &[GlobalVarInfo],
) -> Result<()> {
    emit_saw_specs_with_mode(specs, output_dir, EmitMode::Llvm, experimental, all_globals)
}

/// Emit MIR SAW spec files for all constraints.
pub fn emit_mir_saw_specs(specs: &[SpecConstraint], output_dir: &Path) -> Result<()> {
    emit_saw_specs_with_mode(specs, output_dir, EmitMode::Mir, false, &[])
}

/// Emit a single experimental (llvm_unspecified_globals) spec file.
///
/// If `is_system` is true (e.g. `printf` from `<cstdio>`), the spec will NOT
/// list user-defined globals as modifiable.  System functions only affect
/// external state (stdout, errno, etc.) that SAW doesn't model — assuming
/// they havoc user globals leads to spurious counterexamples.
pub fn emit_single_experimental_spec(
    spec: &SpecConstraint,
    all_globals: &[GlobalVarInfo],
    is_system: bool,
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let safe_id = spec_safe_id(spec);
    let filename = format!("{}_auto_spec.saw", safe_id);
    let filepath = output_dir.join(&filename);
    let effective_globals: &[GlobalVarInfo] = if is_system { &[] } else { all_globals };
    let content = generate_unspecified_spec(spec, effective_globals);
    fs::write(&filepath, content)?;
    Ok(())
}

/// Write the operator new spec to specs_experimental/operator_new_auto_spec.saw
pub fn emit_operator_new_spec(
    mangled_name: &str,
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let filepath = output_dir.join("operator_new_auto_spec.saw");
    // Allocate a generously-sized byte buffer. SAW override matching is
    // satisfied when the actual allocation is at least as large as a
    // constructor override's precondition requires. The previous size of
    // 8 bytes only covered pure-interface classes (vptr only) and caused
    // ctor overrides for classes with data members to fail to match. 256
    // bytes covers every reasonable C++ object size in the demos.
    let content = format!(
        "// Auto-generated spec for: operator new\n\
         // Allocates fresh memory and returns a pointer\n\
         \n\
         let operator_new_spec = do {{\n\
         \x20   sz <- llvm_fresh_var \"sz\" (llvm_int 64);\n\
         \x20   llvm_execute_func [llvm_term sz];\n\
         \x20   p <- llvm_alloc_aligned 8 (llvm_array 256 (llvm_int 8));\n\
         \x20   llvm_return p;\n\
         }};\n\
         ov_new <- llvm_unsafe_assume_spec m \"{}\" operator_new_spec;\n",
        mangled_name,
    );
    fs::write(&filepath, content)?;
    Ok(())
}

pub fn emit_interface_factory_spec(
    spec: &SpecConstraint,
    interface_class: &str,
    all_globals: &[GlobalVarInfo],
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let override_id = spec_safe_id(spec);
    let interface_id = sanitize_name(interface_class).to_lowercase();
    let target_symbol = spec
        .mangled_name
        .as_deref()
        .unwrap_or(&spec.function_name);
    let filepath = output_dir.join(format!("{}_auto_spec.saw", override_id));

    let mut out = String::new();
    out.push_str(&format!(
        "// Factory spec for {}: returns {interface_class}* wired to {interface_id}_vtable.\n",
        spec.function_name,
    ));
    out.push_str("// Must be included after interface_overrides.saw.\n\n");

    out.push_str(&format!(
        "let {override_id}_factory : LLVMSetup () = do {{\n"
    ));

    let mut execute_arguments: Vec<String> = Vec::new();
    for param in &spec.params {
        match param.alloc_type {
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {} <- llvm_fresh_var \"{}\" ({});\n",
                    param.name, param.name, param.saw_type,
                ));
                execute_arguments.push(format!("llvm_term {}", param.name));
            }
            AllocType::AllocReadonly | AllocType::AllocMutable => {
                let allocator = if param.alloc_type == AllocType::AllocReadonly {
                    "llvm_alloc_readonly"
                } else {
                    "llvm_alloc"
                };
                let pointer_name = format!("{}_ptr", param.name);
                out.push_str(&format!(
                    "    {pointer_name} <- {allocator} ({});\n",
                    param.saw_type,
                ));
                execute_arguments.push(pointer_name);
            }
        }
    }

    out.push_str(&format!(
        "\n    llvm_execute_func [{}];\n\n",
        execute_arguments.join(", "),
    ));

    if !all_globals.is_empty() {
        for global in all_globals {
            let width_bits = match &global.ty {
                TypeInfo::SignedInt(width) | TypeInfo::UnsignedInt(width) => *width,
                TypeInfo::Bool => 1,
                _ => 32,
            };
            let post_name = format!("{}_post", sanitize_name(&global.name));
            out.push_str(&format!(
                "    {post_name} <- llvm_fresh_var \"{post_name}\" (llvm_int {width_bits});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {post_name});\n",
                global.mangled_name,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "    this_pointer <- alloc_{interface_id}_this;\n"
    ));
    out.push_str("    llvm_return this_pointer;\n");
    out.push_str("};\n");

    out.push_str(&format!(
        "ov_{override_id} <- llvm_unsafe_assume_spec m \"{target_symbol}\" {override_id}_factory;\n",
    ));

    fs::write(&filepath, out)?;
    Ok(())
}

fn emit_saw_specs_with_mode(
    specs: &[SpecConstraint],
    output_dir: &Path,
    mode: EmitMode,
    experimental: bool,
    all_globals: &[GlobalVarInfo],
) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory {}", output_dir.display()))?;

    for spec in specs {
        let filename = format!("{}_auto_spec.saw", sanitize_name(&spec.function_name));
        let filepath = output_dir.join(&filename);
        let mut file = fs::File::create(&filepath)
            .with_context(|| format!("Failed to create {}", filepath.display()))?;

        let needs_adversarial = spec.is_virtual || !spec.has_body;
        let content = if experimental && needs_adversarial && mode == EmitMode::Llvm {
            generate_unspecified_spec(spec, all_globals)
        } else {
            match mode {
                EmitMode::Llvm => generate_saw_spec(spec),
                EmitMode::Mir => generate_mir_spec(spec),
            }
        };
        write!(file, "{}", content)?;
    }

    // Generate an index file that includes all specs
    let index_path = output_dir.join("auto_specs.saw");
    let mut index = fs::File::create(&index_path)?;
    writeln!(index, "// Auto-generated SAW specs -- do not edit")?;
    writeln!(
        index,
        "// Generated by saw-spec-gen from type system constraints"
    )?;
    writeln!(
        index,
        "// Mode: {}",
        match mode {
            EmitMode::Llvm => "LLVM",
            EmitMode::Mir => "MIR",
        }
    )?;
    writeln!(index)?;
    let mut seen = std::collections::HashSet::new();
    for spec in specs {
        let filename = format!("{}_auto_spec.saw", sanitize_name(&spec.function_name));
        if seen.insert(filename.clone()) {
            writeln!(index, "include \"{filename}\";")?;
        }
    }

    Ok(())
}

fn generate_saw_spec(spec: &SpecConstraint) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;
    let target_name = spec
        .mangled_name
        .as_deref()
        .unwrap_or(&spec.function_name);

    out.push_str(&format!("// Auto-generated spec for: {fn_name}\n"));
    out.push_str("// Constraints derived from type system and annotations\n");
    if spec.can_throw {
        out.push_str("// WARNING: Function may throw -- exception path not modeled\n");
    }
    out.push_str(&format!(
        "\nlet {safe_name}_auto_spec : LLVMSetup () = do {{\n",
        safe_name = sanitize_name(fn_name),
    ));

    // Emit parameter setup
    for param in &spec.params {
        out.push_str(&format!("\n    // Parameter: {}\n", param.name));

        match param.alloc_type {
            AllocType::AllocReadonly => {
                out.push_str(&format!(
                    "    {name}_ptr <- llvm_alloc_readonly ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    {name}_before <- llvm_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {name}_ptr (llvm_term {name}_before);\n",
                    name = param.name,
                ));
            }
            AllocType::AllocMutable => {
                out.push_str(&format!(
                    "    {name}_ptr <- llvm_alloc ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    {name}_val <- llvm_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    llvm_points_to {name}_ptr (llvm_term {name}_val);\n",
                    name = param.name,
                ));
            }
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {name} <- llvm_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
            }
        }

        for pre in &param.preconditions {
            out.push_str(&format!("    {pre};\n"));
        }
    }

    // Emit global variable setup
    if !spec.referenced_globals.is_empty() {
        out.push_str("\n    // Referenced global variables (initialized to compile-time values)\n");
        for global in &spec.referenced_globals {
            let safe_name = sanitize_name(&global.name);
            let saw_type = type_to_saw(&global.ty);
            out.push_str(&format!(
                "    llvm_alloc_global \"{}\";\n",
                global.mangled_name,
            ));
            if let Some(ref init_val) = global.init_value {
                // Use the compile-time initial value
                let bits = match &global.ty {
                    TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => *b,
                    TypeInfo::Bool => 1,
                    _ => 32,
                };
                out.push_str(&format!(
                    "    llvm_points_to (llvm_global \"{}\") (llvm_term {{{{ {} : [{}] }}}});\n",
                    global.mangled_name, init_val, bits,
                ));
            } else {
                // No known initial value — use fresh var
                out.push_str(&format!(
                    "    {safe_name}_global <- llvm_fresh_var \"{safe_name}\" ({saw_type});\n",
                ));
                out.push_str(&format!(
                    "    llvm_points_to (llvm_global \"{}\") (llvm_term {safe_name}_global);\n",
                    global.mangled_name,
                ));
            }
        }
    }

    // Emit execute_func
    let mut args: Vec<String> = Vec::new();

    // sret: allocate the return struct as a mutable pointer, prepend to args
    if spec.return_constraint.is_sret {
        out.push_str(&format!(
            "\n    // sret: struct returned via hidden first pointer parameter\n"
        ));
        out.push_str(&format!(
            "    result_ptr <- llvm_alloc ({});\n",
            spec.return_constraint.saw_type,
        ));
        args.push("result_ptr".into());
    }

    args.extend(spec
        .params
        .iter()
        .map(|p| match p.alloc_type {
            AllocType::AllocReadonly | AllocType::AllocMutable => format!("{}_ptr", p.name),
            AllocType::FreshVar => format!("llvm_term {}", p.name),
        }));
    out.push_str(&format!(
        "\n    llvm_execute_func [{}];\n",
        args.join(", ")
    ));

    // Use adversarial model when implementation is unavailable:
    // virtual methods (vtable dispatch) or external functions (no body / stdlib)
    let needs_adversarial = spec.is_virtual || !spec.has_body;

    // Emit return value and postconditions
    if needs_adversarial {
        // ── Constrained adversarial model ──
        // The implementation is unknown, so we model the most adversarial
        // behavior the type system allows:
        //   • const / _In_ params  → PRESERVED (unchanged after call)
        //   • mutable / _Out_ params → HAVOCED (solver picks any post-call value)
        //   • return value          → unconstrained (solver picks any value)

        // Return: unconstrained fresh var
        if spec.return_constraint.returns_pointer {
            // Function returns a pointer (e.g. operator new) — allocate memory
            out.push_str(&format!(
                "\n    ret_ptr <- llvm_alloc ({});\n",
                spec.return_constraint.saw_type,
            ));
            out.push_str("    llvm_return ret_ptr;\n");
        } else if spec.return_constraint.is_sret {
            out.push_str(&format!(
                "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
                spec.return_constraint.saw_type,
            ));
            for vc in &spec.return_constraint.value_constraints {
                out.push_str(&format!("    {vc};\n"));
            }
            out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
        } else if spec.return_constraint.saw_type != "// void" {
            out.push_str(&format!(
                "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
                spec.return_constraint.saw_type,
            ));
            for vc in &spec.return_constraint.value_constraints {
                out.push_str(&format!("    {vc};\n"));
            }
            out.push_str("    llvm_return (llvm_term ret);\n");
        }

        // Adversarial postconditions: preserved vs havoced per-parameter
        out.push_str("\n    // Adversarial postconditions\n");
        for param in &spec.params {
            match param.alloc_type {
                AllocType::AllocReadonly => {
                    // Const param: preserved (same value after call)
                    out.push_str(&format!(
                        "    // {name}: PRESERVED (const)\n",
                        name = param.name,
                    ));
                    out.push_str(&format!(
                        "    llvm_points_to {name}_ptr (llvm_term {name}_before);\n",
                        name = param.name,
                    ));
                }
                AllocType::AllocMutable => {
                    // Mutable param: havoced (solver picks any post-call value)
                    out.push_str(&format!(
                        "    // {name}: HAVOCED (mutable)\n",
                        name = param.name,
                    ));
                    out.push_str(&format!(
                        "    {name}_after <- llvm_fresh_var \"{name}_after\" ({ty});\n",
                        name = param.name,
                        ty = param.saw_type,
                    ));
                    out.push_str(&format!(
                        "    llvm_points_to {name}_ptr (llvm_term {name}_after);\n",
                        name = param.name,
                    ));
                }
                AllocType::FreshVar => {
                    // Pass-by-value: no memory to havoc
                }
            }
        }
    } else {
        // ── Concrete function: verify against actual implementation ──
        // Return: unconstrained fresh var — llvm_verify will check the real
        // implementation satisfies this. Replace with a Cryptol postcondition
        // (e.g. llvm_return (llvm_term {{ my_spec x }})) for functional correctness.
        if spec.return_constraint.returns_pointer {
            // Function returns a pointer — allocate memory for the return
            out.push_str(&format!(
                "\n    ret_ptr <- llvm_alloc ({});\n",
                spec.return_constraint.saw_type,
            ));
            out.push_str("    llvm_return ret_ptr;\n");
        } else if spec.return_constraint.is_sret {
            out.push_str(&format!(
                "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
                spec.return_constraint.saw_type,
            ));
            for vc in &spec.return_constraint.value_constraints {
                out.push_str(&format!("    {vc};\n"));
            }
            out.push_str("    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n");
            out.push_str("    // For functional correctness, import your Cryptol spec and replace ret:\n");
            out.push_str("    //   import \"my_spec.cry\";\n");
            out.push_str("    //   llvm_points_to result_ptr (llvm_term {{ my_spec x }});\n");
            out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
        } else if spec.return_constraint.saw_type != "// void" {
            out.push_str(&format!(
                "\n    ret <- llvm_fresh_var \"ret\" ({});\n",
                spec.return_constraint.saw_type,
            ));
            for vc in &spec.return_constraint.value_constraints {
                out.push_str(&format!("    {vc};\n"));
            }
            out.push_str(&format!(
                "    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n"
            ));
            out.push_str(&format!(
                "    // For functional correctness, import your Cryptol spec and replace ret:\n"
            ));
            out.push_str(&format!(
                "    //   import \"my_spec.cry\";\n"
            ));
            out.push_str(&format!(
                "    //   llvm_return (llvm_term {{{{ my_spec x : {} }}}});\n",
                cryptol_type_for_saw(&spec.return_constraint.saw_type),
            ));
            out.push_str("    llvm_return (llvm_term ret);\n");
        }

        // Emit existing postconditions (readonly params unchanged, annotations)
        if !spec.postconditions.is_empty() {
            out.push_str("\n    // Postconditions\n");
            for post in &spec.postconditions {
                out.push_str(&format!("    {post};\n"));
            }
        }
    }

    out.push_str("};\n");

    // Add the usage hint
    if needs_adversarial {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- llvm_unsafe_assume_spec m \"{target}\" {safe}_auto_spec;\n",
            safe = sanitize_name(fn_name),
            target = target_name,
        ));
    } else {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- llvm_verify m \"{target}\" [] false {safe}_auto_spec z3;\n",
            safe = sanitize_name(fn_name),
            target = target_name,
        ));
    }

    out
}

/// Generate a spec using SAW's experimental llvm_unspecified_globals builtin.
///
/// This produces a much simpler spec for virtual/external functions:
/// SAW automatically generates fresh symbolic values for all outputs.
fn generate_unspecified_spec(spec: &SpecConstraint, all_globals: &[GlobalVarInfo]) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;
    let target_name = spec
        .mangled_name
        .as_deref()
        .unwrap_or(&spec.function_name);

    out.push_str(&format!("// Auto-generated spec for: {fn_name}\n"));
    out.push_str("// Uses SAW experimental llvm_unspecified_globals builtin\n");
    out.push_str("// Requires: enable_experimental;\n\n");

    // Use all globals from the translation unit — any external/virtual function
    // could modify any accessible global.
    let safe = spec_safe_id(spec);
    if all_globals.is_empty() {
        out.push_str(&format!(
            "ov_{safe} <- llvm_unspecified m \"{target}\";\n",
            target = target_name,
        ));
    } else {
        let globals_list = all_globals
            .iter()
            .map(|g| format!("\"{}\"", g.mangled_name))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "ov_{safe} <- llvm_unspecified_globals m \"{target}\" [{globals_list}];\n",
            target = target_name,
        ));
    }

    out
}

fn generate_mir_spec(spec: &SpecConstraint) -> String {
    let mut out = String::new();
    let fn_name = &spec.function_name;

    out.push_str(&format!("// Auto-generated MIR spec for: {fn_name}\n"));
    out.push_str("// Constraints derived from Rust type system\n");
    out.push_str(&format!(
        "\nlet {safe_name}_auto_spec : MIRSetup () = do {{\n",
        safe_name = sanitize_name(fn_name),
    ));

    // Emit parameter setup using MIR commands
    for param in &spec.params {
        out.push_str(&format!("\n    // Parameter: {}\n", param.name));

        match param.alloc_type {
            AllocType::AllocReadonly => {
                out.push_str(&format!(
                    "    {name}_ref <- mir_alloc_readonly (mir_find_adt m \"{ty}\");\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    {name}_before <- mir_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = mir_saw_type(&param.saw_type),
                ));
                out.push_str(&format!(
                    "    mir_points_to {name}_ref (mir_term {name}_before);\n",
                    name = param.name,
                ));
            }
            AllocType::AllocMutable => {
                out.push_str(&format!(
                    "    {name}_ref <- mir_alloc (mir_find_adt m \"{ty}\");\n",
                    name = param.name,
                    ty = param.saw_type,
                ));
                out.push_str(&format!(
                    "    {name}_val <- mir_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = mir_saw_type(&param.saw_type),
                ));
                out.push_str(&format!(
                    "    mir_points_to {name}_ref (mir_term {name}_val);\n",
                    name = param.name,
                ));
            }
            AllocType::FreshVar => {
                out.push_str(&format!(
                    "    {name} <- mir_fresh_var \"{name}\" ({ty});\n",
                    name = param.name,
                    ty = mir_saw_type(&param.saw_type),
                ));
            }
        }

        for pre in &param.preconditions {
            let mir_pre = pre
                .replace("llvm_precond", "mir_precond")
                .replace("llvm_", "mir_");
            out.push_str(&format!("    {mir_pre};\n"));
        }
    }

    // Emit execute_func
    let args: Vec<String> = spec
        .params
        .iter()
        .map(|p| match p.alloc_type {
            AllocType::AllocReadonly | AllocType::AllocMutable => format!("{}_ref", p.name),
            AllocType::FreshVar => format!("mir_term {}", p.name),
        })
        .collect();
    out.push_str(&format!(
        "\n    mir_execute_func [{}];\n",
        args.join(", ")
    ));

    // Emit return value and postconditions
    let needs_adversarial = spec.is_virtual || !spec.has_body;
    if needs_adversarial {
        // ── Constrained adversarial model for virtual/dyn/external methods ──
        // Return: unconstrained fresh var
        if spec.return_constraint.saw_type != "// void" {
            out.push_str(&format!(
                "\n    ret <- mir_fresh_var \"ret\" ({});\n",
                mir_saw_type(&spec.return_constraint.saw_type),
            ));
            for vc in &spec.return_constraint.value_constraints {
                let mir_vc = vc
                    .replace("llvm_postcond", "mir_postcond")
                    .replace("llvm_", "mir_");
                out.push_str(&format!("    {mir_vc};\n"));
            }
            out.push_str("    mir_return (mir_term ret);\n");
        }

        // Adversarial postconditions: preserved vs havoced per-parameter
        out.push_str("\n    // Adversarial postconditions\n");
        for param in &spec.params {
            match param.alloc_type {
                AllocType::AllocReadonly => {
                    out.push_str(&format!(
                        "    // {name}: PRESERVED (immutable ref)\n",
                        name = param.name,
                    ));
                    out.push_str(&format!(
                        "    mir_points_to {name}_ref (mir_term {name}_before);\n",
                        name = param.name,
                    ));
                }
                AllocType::AllocMutable => {
                    out.push_str(&format!(
                        "    // {name}: HAVOCED (mutable ref)\n",
                        name = param.name,
                    ));
                    out.push_str(&format!(
                        "    {name}_after <- mir_fresh_var \"{name}_after\" ({ty});\n",
                        name = param.name,
                        ty = mir_saw_type(&param.saw_type),
                    ));
                    out.push_str(&format!(
                        "    mir_points_to {name}_ref (mir_term {name}_after);\n",
                        name = param.name,
                    ));
                }
                AllocType::FreshVar => {}
            }
        }
    } else {
        // ── Concrete function: verify against actual implementation ──
        if spec.return_constraint.saw_type != "// void" {
            out.push_str(&format!(
                "\n    ret <- mir_fresh_var \"ret\" ({});\n",
                mir_saw_type(&spec.return_constraint.saw_type),
            ));
            for vc in &spec.return_constraint.value_constraints {
                let mir_vc = vc
                    .replace("llvm_postcond", "mir_postcond")
                    .replace("llvm_", "mir_");
                out.push_str(&format!("    {mir_vc};\n"));
            }
            out.push_str(&format!(
                "    // ACTION REQUIRED: This spec only proves absence of undefined behavior.\n"
            ));
            out.push_str(&format!(
                "    // For functional correctness, import your Cryptol spec and replace ret:\n"
            ));
            out.push_str(&format!(
                "    //   mir_return (mir_term {{{{ my_spec x : {} }}}});\n",
                cryptol_type_for_saw(&spec.return_constraint.saw_type),
            ));
            out.push_str("    mir_return (mir_term ret);\n");
        }

        // Emit existing postconditions
        if !spec.postconditions.is_empty() {
            out.push_str("\n    // Postconditions\n");
            for post in &spec.postconditions {
                let mir_post = post
                    .replace("llvm_points_to", "mir_points_to")
                    .replace("llvm_term", "mir_term");
                out.push_str(&format!("    {mir_post};\n"));
            }
        }
    }

    out.push_str("};\n");

    // Add the usage hint
    if needs_adversarial {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- mir_unsafe_assume_spec m \"{target}\" {safe}_auto_spec;\n",
            safe = sanitize_name(fn_name),
            target = fn_name,
        ));
    } else {
        out.push_str(&format!(
            "\n// Usage: ov_{safe} <- mir_verify m \"{target}\" [] false {safe}_auto_spec z3;\n",
            safe = sanitize_name(fn_name),
            target = fn_name,
        ));
    }

    out
}

/// Map LLVM SAW type syntax to MIR type syntax.
fn mir_saw_type(llvm_type: &str) -> String {
    if llvm_type.starts_with("llvm_int ") {
        let bits = llvm_type.trim_start_matches("llvm_int ");
        format!("mir_u{bits}")
    } else if llvm_type.starts_with("llvm_array ") {
        llvm_type
            .replace("llvm_array", "mir_array")
            .replace("llvm_int", "mir_u")
    } else if llvm_type.starts_with("llvm_alias ") {
        let name = llvm_type
            .trim_start_matches("llvm_alias \"")
            .trim_end_matches('"');
        format!("mir_find_adt m \"{name}\"")
    } else {
        llvm_type.to_string()
    }
}

fn sanitize_name(name: &str) -> String {
    // If the input looks like a Rust v0 or Itanium mangled symbol, replace
    // it with a short human-readable form plus a stable hash for
    // collision resistance. Anything else goes through the legacy
    // alphanum-only path.
    if let Some(human) = crate::mangle::humanize(name) {
        let safe = crate::mangle::sanitize_filename_chars(&human);
        let hash = crate::mangle::short_hash(name);
        return format!("{safe}_{hash}");
    }
    let sanitized = crate::mangle::sanitize_filename_chars(name);
    // Truncate to avoid exceeding filesystem limits (Windows MAX_PATH).
    // If truncated, append a short hash to keep names unique.
    if sanitized.len() > 120 {
        let hash = crate::mangle::short_hash(&sanitized);
        format!("{}_{}", &sanitized[..120], hash)
    } else {
        sanitized
    }
}

/// Stable identifier for a spec — used for filename, override variable, and
/// include statement.  Prefer the mangled name to avoid collisions between
/// overloads or methods that share an unmangled name (e.g. `log`).
fn spec_safe_id(spec: &SpecConstraint) -> String {
    if let Some(ref m) = spec.mangled_name {
        if !m.is_empty() && m != &spec.function_name {
            return sanitize_name(m);
        }
    }
    sanitize_name(&spec.function_name)
}

/// Map SAW type syntax to Cryptol type syntax for use in TODO comments.
fn cryptol_type_for_saw(saw_type: &str) -> String {
    // llvm_int N -> [N]
    if saw_type.starts_with("llvm_int ") {
        let bits = saw_type.strip_prefix("llvm_int ").unwrap_or("32");
        format!("[{bits}]")
    } else {
        saw_type.to_string()
    }
}

// ============================================================================
// Interface stub + havoc spec generation
// ============================================================================

/// Emit vtable resolution stubs and havoc SAW specs for virtual methods.
///
/// Generates:
/// - `vtable_stubs.c`: C file with stub functions + concrete vtable globals per class
/// - `*_havoc_spec.saw`: SAW havoc specs where mutable memory is explicitly havoced
/// - `interface_overrides.saw`: Override setup + helper to allocate `this` with vtable wired
/// - Constructor override specs that wire vptr to stub vtable for internal object construction
pub fn emit_interface_stubs(
    methods: &[InterfaceMethod],
    constructors: &[ClassConstructor],
    globals: &[GlobalVarInfo],
    classes_with_vdtor: &std::collections::HashSet<String>,
    output_dir: &Path,
    cryptol_fn: Option<&str>,
) -> Result<()> {
    if methods.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create {}", output_dir.display()))?;

    // Group methods by class for vtable generation.
    //
    // Each class's method list MUST be sorted by source-location offset so
    // vtable slots are assigned in declaration order — MSVC (and the
    // Itanium ABI) order virtual table slots by the order methods appear
    // in the class body, not by the order the AST walker happens to
    // encounter them. Sorting by `source_offset` here is what keeps the
    // generated vtable indices in sync with the real ABI; otherwise SAW
    // resolves vtable dispatch to the wrong stub function.
    let mut by_class: std::collections::BTreeMap<String, Vec<&InterfaceMethod>> =
        std::collections::BTreeMap::new();
    for method in methods {
        by_class
            .entry(method.class_name.clone())
            .or_default()
            .push(method);
    }
    for class_methods in by_class.values_mut() {
        class_methods.sort_by_key(|m| m.source_offset);
    }

    // Generate the LLVM IR stub file (vtable_stubs.ll) — no C compiler needed
    let ll_path = output_dir.join("vtable_stubs.ll");
    let ll_content = generate_llvm_ir_stubs(&by_class, classes_with_vdtor);
    fs::write(&ll_path, &ll_content)
        .with_context(|| format!("Failed to write {}", ll_path.display()))?;

    // Generate havoc SAW specs — one per *originating* method only.
    // Overrides reuse their base class's stub function in the vtable, so
    // generating a separate havoc spec for the derived class would refer
    // to a stub symbol that doesn't exist.
    //
    // Build a lookup `class_name -> ClassConstructor` so each havoc spec
    // can refer to its parent class's full layout. This is what lets a
    // `const` method on a class with a `mutable` field correctly havoc
    // the field in the postcondition.
    let layout_by_class: std::collections::HashMap<&str, &ClassConstructor> = constructors
        .iter()
        .map(|c| (c.class_name.as_str(), c))
        .collect();
    for method in methods {
        if method.is_override {
            continue;
        }
        let filename = format!(
            "{}_{}_havoc_spec.saw",
            sanitize_name(&method.class_name),
            sanitize_name(&method.method.name),
        );
        let filepath = output_dir.join(&filename);
        let layout = layout_by_class.get(method.class_name.as_str()).copied();
        let content = generate_havoc_spec(method, globals, layout, cryptol_fn);
        fs::write(&filepath, content)?;
    }

    // Generate the index/override file with vtable helpers and constructor overrides
    let index_path = output_dir.join("interface_overrides.saw");
    let index_content =
        generate_override_index_with_vtable(methods, &by_class, constructors, classes_with_vdtor);
    fs::write(&index_path, index_content)?;

    Ok(())
}

/// Try to assemble `vtable_stubs.ll` in `output_dir` into `vtable_stubs.bc`.
///
/// SAW's `llvm_load_module` only accepts LLVM bitcode (`.bc`) — passing it a
/// `.ll` text file fails with "Invalid magic number" because SAW tries to
/// parse the file as bitcode. To make the generated `verify.saw` runnable
/// without an external build step, we invoke `llvm-as` (preferred) or fall
/// back to `clang -c -emit-llvm` to assemble the `.ll` we just wrote.
///
/// Returns the basename (with extension) of the stubs file that downstream
/// code should reference in `llvm_load_module`. If neither assembler is
/// available, returns `Some("vtable_stubs.ll")` together with a warning —
/// the caller can then emit a clearer build-step comment in the script.
///
/// Returns `None` when `vtable_stubs.ll` doesn't exist in `output_dir`
/// (i.e. there were no virtual methods, so no stubs were generated).
pub fn assemble_vtable_stubs(output_dir: &Path) -> AssembledStubs {
    let ll_path = output_dir.join("vtable_stubs.ll");
    if !ll_path.exists() {
        return AssembledStubs::NoStubs;
    }
    let bc_path = output_dir.join("vtable_stubs.bc");

    // Try llvm-as first (single-purpose tool, fast, ubiquitous in LLVM
    // installs). Fall back to clang's -emit-llvm so users with only a
    // bundled clang on PATH (typical MSVC-only setups) still get a .bc.
    let candidates: &[(&str, &[&str])] = &[
        ("llvm-as", &[]),
        ("llvm-as.exe", &[]),
        ("clang", &["-c", "-emit-llvm"]),
        ("clang.exe", &["-c", "-emit-llvm"]),
        ("clang-cl", &["-c", "-emit-llvm"]),
        ("clang-cl.exe", &["-c", "-emit-llvm"]),
    ];
    for (cmd, extra_args) in candidates {
        match std::process::Command::new(cmd)
            .args(*extra_args)
            .arg(&ll_path)
            .arg("-o")
            .arg(&bc_path)
            .output()
        {
            Ok(out) if out.status.success() && bc_path.exists() => {
                return AssembledStubs::Bitcode {
                    bc_filename: "vtable_stubs.bc".to_string(),
                    assembler: (*cmd).to_string(),
                };
            }
            _ => continue,
        }
    }
    AssembledStubs::TextOnly {
        ll_filename: "vtable_stubs.ll".to_string(),
    }
}

/// Result of `assemble_vtable_stubs`.
#[derive(Debug, Clone)]
pub enum AssembledStubs {
    /// `vtable_stubs.ll` was not present in the output directory; nothing
    /// to assemble. Callers should emit a script that loads only the
    /// main bitcode.
    NoStubs,
    /// The text IR was successfully assembled to bitcode. `bc_filename`
    /// should be referenced from `llvm_load_module` in the generated
    /// script.
    Bitcode { bc_filename: String, assembler: String },
    /// No assembler was available on PATH. The script will reference the
    /// `.ll` file, which SAW cannot load — the user has to assemble it
    /// manually. Callers should print a build-step instruction.
    TextOnly { ll_filename: String },
}

impl AssembledStubs {
    /// Filename (basename, including extension) for `llvm_load_module` to
    /// reference. Returns `None` when no stubs file exists.
    ///
    /// Always returns `vtable_stubs.bc` whenever stubs were generated —
    /// even when auto-assembly failed and only `vtable_stubs.ll` is
    /// present on disk. SAW's `llvm_load_module` only accepts bitcode,
    /// so the generated script must reference the `.bc` filename. The
    /// emitted script prints a warning instructing the user to assemble
    /// the `.ll` to `.bc` before running SAW when auto-assembly failed.
    pub fn script_filename(&self) -> Option<&str> {
        match self {
            AssembledStubs::NoStubs => None,
            AssembledStubs::Bitcode { bc_filename, .. } => Some(bc_filename.as_str()),
            AssembledStubs::TextOnly { .. } => Some("vtable_stubs.bc"),
        }
    }
}

/// Generate LLVM IR text (.ll) with stub functions and vtable globals.
///
/// This is directly loadable by SAW via `llvm_load_module "vtable_stubs.ll"`
/// or assembleable via `llvm-as vtable_stubs.ll -o vtable_stubs.bc`.
/// No C compiler needed.
///
/// Override methods (carrying `OverrideAttr` in the AST) do NOT get their
/// own stub function. Instead, the derived class's vtable slot reuses the
/// stub of the class that first declared the method. This way every class
/// in a hierarchy stores the *same function pointer* at the inherited
/// vtable slot, so when SAW symbolically merges across multiple derived
/// types, the resulting indirect call is still concrete.
fn generate_llvm_ir_stubs(
    by_class: &std::collections::BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &std::collections::HashSet<String>,
) -> String {
    // Build an index: method name -> originating class (first non-override
    // declaration found across the whole hierarchy). Used to redirect
    // override vtable slots to the base's stub.
    let mut originating: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (_, methods) in by_class {
        for m in methods {
            if !m.is_override {
                originating
                    .entry(m.method.name.clone())
                    .or_insert_with(|| m.class_name.clone());
            }
        }
    }
    // Helper: which class's stub should this method's vtable slot reference?
    // For non-overrides: the class itself. For overrides: the originating class
    // (or, if none was found, fall back to the class itself so we still emit
    // a valid LLVM module).
    let resolve_stub_class = |m: &InterfaceMethod| -> String {
        if m.is_override {
            originating
                .get(&m.method.name)
                .cloned()
                .unwrap_or_else(|| m.class_name.clone())
        } else {
            m.class_name.clone()
        }
    };
    let stub_name_for = |m: &InterfaceMethod| -> String {
        let cls = resolve_stub_class(m);
        format!(
            "{}_{}_stub",
            sanitize_name(&cls).to_lowercase(),
            sanitize_name(&m.method.name).to_lowercase(),
        )
    };

    let mut out = String::new();
    out.push_str("; Auto-generated vtable stubs for SAW verification\n");
    out.push_str("; Load directly: m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("; Or assemble:   llvm-as vtable_stubs.ll -o vtable_stubs.bc\n");
    out.push_str(";\n");
    out.push_str("; SAW resolves indirect vtable calls through these:\n");
    out.push_str(";   this->vptr -> vtable[slot] -> stub function -> havoc spec\n\n");
    out.push_str("target datalayout = \"e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"\n");
    out.push_str("target triple = \"x86_64-pc-windows-msvc\"\n\n");

    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();
        let has_vdtor = classes_with_vdtor.contains(class_name.as_str());

        out.push_str(&format!("; ---- {class_name} vtable ----\n\n"));

        // MSVC vtable layout (clang --target=x86_64-pc-windows-msvc):
        // - With virtual dtor: [deleting_dtor, method1, method2, ...]
        // - Without virtual dtor (implicit/abstract): [method1, method2, ...]
        //
        // NOTE: This is the MSVC ABI, NOT the Itanium/GCC ABI. Itanium
        // vtables carry BOTH a complete-object destructor and a
        // deleting destructor; MSVC carries only the deleting
        // destructor (mangled `??_G...`) at slot 0, taking
        // (ptr %this, i32 %flags). Emitting a second dtor slot would
        // shift every subsequent method by one and cause SAW to
        // dispatch through the wrong vtable entry.
        if has_vdtor {
            // Deleting destructor stub (slot 0) — takes (ptr, i32), returns void
            out.push_str(&format!(
                "define void @{safe_class}_deleting_dtor_stub(ptr %self, i32 %flags) {{\n  ret void\n}}\n\n"
            ));
        }

        // Method stubs — emit only for methods this class originates.
        // Overrides share the originating class's stub (emitted above when
        // that class was processed; `by_class` is a BTreeMap so iteration is
        // deterministic but not topological, so we always emit each method's
        // stub when we visit its originating class).
        for method in class_methods {
            if method.is_override {
                continue;
            }
            let stub_name = stub_name_for(method);
            let param_strs = method_param_ir_pieces(&method.method);

            // MSVC ABI: any return type the platform can't fit in registers
            // (struct/class larger than 8 bytes) is lowered to a hidden
            // sret pointer parameter. For instance methods the parameter
            // order is `this, sret_ptr, args...` — the implicit `this`
            // stays at position 0 and the output pointer is inserted at
            // position 1. The stub signature must match this ABI or SAW
            // will reject the generated module when it links against
            // bitcode that calls the real virtual method.
            let (ret_ir, params_ir) = match sret_inner_ir_type(&method.method.return_type) {
                Some(inner) => {
                    let mut parts: Vec<String> = Vec::new();
                    // `this` always lives at param index 0 for an
                    // InterfaceMethod (added by the AST walker). Keep it
                    // first; insert the sret pointer immediately after.
                    if let Some(this_p) = param_strs.first() {
                        parts.push(this_p.clone());
                    }
                    parts.push(format!("ptr sret({inner}) %retptr"));
                    for p in param_strs.iter().skip(1) {
                        parts.push(p.clone());
                    }
                    ("void".to_string(), parts.join(", "))
                }
                None => (
                    type_to_llvm_ir(&method.method.return_type),
                    param_strs.join(", "),
                ),
            };

            out.push_str(&format!(
                "; {class_name}::{} [{}]\n",
                method.method.name,
                if method.is_pure { "pure virtual" } else { "virtual" },
            ));
            out.push_str(&format!(
                "define {ret_ir} @{stub_name}({params_ir}) {{\n"
            ));
            out.push_str(&format!("  {}\n", ir_default_return(&ret_ir)));
            out.push_str("}\n\n");
        }

        // Vtable global — every slot points at the stub of the *originating*
        // class for that method. For inherited methods this means the
        // derived class's vtable references the base class's stub directly.
        // MSVC ABI: exactly one dtor slot (the deleting destructor) when
        // the class has a virtual dtor; no dtor slot otherwise.
        let dtor_slots = if has_vdtor { 1 } else { 0 };
        let slot_count = class_methods.len() + dtor_slots;
        out.push_str(&format!(
            "; Concrete vtable for {class_name}\n"
        ));
        out.push_str(&format!(
            "@{safe_class}_vtable = global [{slot_count} x ptr] [\n"
        ));
        let mut first = true;
        if has_vdtor {
            out.push_str(&format!("  ptr @{safe_class}_deleting_dtor_stub"));
            first = false;
        }
        for method in class_methods {
            let stub_name = stub_name_for(method);
            if first {
                out.push_str(&format!("  ptr @{stub_name}"));
                first = false;
            } else {
                out.push_str(&format!(",\n  ptr @{stub_name}"));
            }
        }
        out.push_str("\n]\n\n");
    }

    out
}

/// Convert a TypeInfo to LLVM IR type string.
fn type_to_llvm_ir(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Void => "void".into(),
        TypeInfo::Bool => "i1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("i{bits}"),
        TypeInfo::Pointer(_) => "ptr".into(),
        TypeInfo::Struct { size_bytes: Some(n), .. } => format!("[{n} x i8]"),
        TypeInfo::ByteArray(n) => format!("[{n} x i8]"),
        TypeInfo::Enum { discriminant_bits, .. } => format!("i{discriminant_bits}"),
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 0 && *size_bytes <= 8 => {
            // Small opaque type — likely an enum; use integer of matching size
            format!("i{}", size_bytes * 8)
        }
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8 => {
            format!("[{size_bytes} x i8]")
        }
        _ => "i32".into(), // default to i32 for unknown small types (likely enums)
    }
}

/// Returns `Some(inner_ir_type)` when a return type is lowered to an sret
/// pointer parameter under the MSVC x64 ABI. The inner type is what goes
/// inside `sret(...)` in the LLVM IR signature.
///
/// MSVC returns aggregates larger than 8 bytes via sret. We conservatively
/// treat any `Struct` (sized or unsized) as sret because aggregate returns
/// in C++ classes always use the hidden pointer convention regardless of
/// whether we have a full layout. Unsized structs fall back to a 16-byte
/// scratch buffer — SAW only needs the signature to match for type
/// checking; the body is unreachable when the spec is applied as an
/// override.
///
/// Unsized `Opaque` returns whose name looks like an aggregate (template
/// or qualified C++ name — e.g. `std::tuple<KeyStoreOperationResult,
/// LatchableKey>`) are also sret. Without LLVM IR sizes the AST has no
/// way to populate `size_bytes`, but the MSVC ABI lowering only depends
/// on whether the type is an aggregate, not on its exact size.
fn sret_inner_ir_type(ty: &TypeInfo) -> Option<String> {
    match ty {
        TypeInfo::Struct { size_bytes: Some(n), .. } => Some(format!("[{n} x i8]")),
        TypeInfo::Struct { size_bytes: None, .. } => Some("[16 x i8]".to_string()),
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 8 => {
            Some(format!("[{size_bytes} x i8]"))
        }
        TypeInfo::Opaque { size_bytes: 0, name } if looks_like_aggregate_name(name) => {
            Some("[16 x i8]".to_string())
        }
        _ => None,
    }
}

/// Heuristic: is this opaque type name almost certainly a C++ aggregate
/// (struct/class/template instantiation) rather than a scalar alias?
///
/// Used only as a fallback when `size_bytes` isn't known. Templated names
/// (`std::tuple<…>`) and fully-qualified C++ names always denote class
/// types under MSVC, which means the ABI returns them via sret. Plain
/// unqualified opaques like `Self` / `Unknown` (used for unresolved
/// abstract `this` placeholders) are NOT aggregates.
fn looks_like_aggregate_name(name: &str) -> bool {
    name.contains('<') || name.contains("::")
}

/// Generate LLVM IR parameter pieces (one per param) for a method's stub.
///
/// Returned as a `Vec<String>` so callers can splice the implicit `this`
/// and an sret pointer at the right positions before joining.
fn method_param_ir_pieces(func: &FunctionInfo) -> Vec<String> {
    func.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match &p.ty {
                TypeInfo::Pointer(_) => "ptr".to_string(),
                other => type_to_llvm_ir(other),
            };
            format!("{ty} %arg{i}")
        })
        .collect()
}

/// Default return instruction for an LLVM IR type.
fn ir_default_return(ir_type: &str) -> String {
    match ir_type {
        "void" => "ret void".into(),
        "i1" => "ret i1 false".into(),
        t => format!("ret {t} 0"),
    }
}

/// Generate an adversarial havoc SAW spec for one virtual method.
///
/// Models the most adversarial behavior consistent with annotations and types:
///
/// **Annotation semantics** (SAL / prefast / const):
///   - `_In_` / `const T*` / `_In_reads_(n)` → preserved (read-only)
///   - `_Out_` / `_Out_writes_(n)` → havoced (write-only, any final value)
///   - `_Inout_` / `T*` (non-const) → havoced (read-write, any final value)
///   - `const` method → `this` is preserved
///   - non-const method → `this` is havoced (all fields get arbitrary values)
///
/// **Struct field decomposition**:
///   - When a struct type has known fields, each field gets its own fresh var
///   - Pointer fields → nested allocation with the same const/mutable semantics
///   - Uses `llvm_struct_value` for field-level precision instead of opaque blobs
///
/// **NOT modeled** (noted in output, requires manual extension):
///   - Pointer chains deeper than 2 levels
///   - Aliased memory not visible in the type signature
fn generate_havoc_spec(
    imethod: &InterfaceMethod,
    globals: &[GlobalVarInfo],
    layout: Option<&ClassConstructor>,
    cryptol_fn: Option<&str>,
) -> String {
    let mut out = String::new();
    let func = &imethod.method;
    let stub_name = stub_function_name(imethod);
    let safe_name = sanitize_name(&stub_name);

    out.push_str(&format!(
        "// Adversarial havoc spec for {}::{}\n",
        imethod.class_name, func.name,
    ));
    out.push_str("//\n");
    out.push_str("// Models ANY implementation consistent with type-system and SAL constraints.\n");
    out.push_str("// The solver can choose ANY value for mutable/writable memory after the call.\n");
    out.push_str("// Const/_In_ memory is preserved — the solver cannot modify it.\n");
    out.push_str("//\n");
    out.push_str("// Annotation key:\n");
    out.push_str("//   _In_ / const    → PRESERVED (read-only, unchanged after call)\n");
    out.push_str("//   _Out_           → HAVOCED   (write-only, any final value)\n");
    out.push_str("//   _Inout_ / &mut  → HAVOCED   (read-write, any final value)\n");
    out.push_str("//   no annotation   → inferred from const/mutable\n");
    out.push_str("//\n");
    out.push_str("// NOT modeled: pointer chains >2 deep, aliased memory.\n\n");

    out.push_str(&format!(
        "let {safe_name}_havoc : LLVMSetup () = do {{\n"
    ));

    let mut setup = String::new();
    let mut postconds = String::new();
    let mut args = Vec::new();

    // Pre-compute the Cryptol postcondition expression for any `_Out_` buffer:
    //   `<cryptol_fn> <input1> <input2> ...`
    // Inputs are scalar params (their fresh-var name) and preserved-pointer
    // params (their `{name}_val` symbolic value). We skip `this` and any
    // havoced/write-only pointers (they are *outputs*, not inputs).
    let cryptol_post_expr: Option<String> = cryptol_fn.map(|fn_name| {
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
    });

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

        // Determine havoc behavior from annotations + type system
        let behavior = resolve_param_behavior(param);

        // Special path for a *mutable* `this` when we know the class
        // layout: allocate the full class struct and havoc the data
        // members after the call. The plain `this`-as-opaque path in
        // `emit_adversarial_param` only allocates 8 bytes (the vptr)
        // and emits no postcondition, so data members at offsets ≥ 8
        // would otherwise stay at their pre-call values. That misses
        // the case where a `const` method modifies a `mutable` member.
        if param.name == "this"
            && behavior == HavocBehavior::Havoced
            && layout
                .map(|c| !c.layout_fields.is_empty())
                .unwrap_or(false)
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
            cryptol_post_expr.as_deref(),
        );
        args.push(format!("{}_ptr", param.name));
    }

    // MSVC ABI sret: an aggregate return is passed via a hidden output
    // pointer at position 1 (immediately after `this`). Allocate the
    // output buffer in setup, splice it into the argument list, and
    // remember the SAW type so the postcondition can constrain *retptr
    // instead of using llvm_return.
    let sret_saw_type: Option<String> = sret_inner_ir_type(&func.return_type).map(|_| {
        match &func.return_type {
            // Unsized opaque aggregate (e.g. std::tuple<…>): use a
            // byte-array scratch buffer matching the IR stub's sret type.
            // type_to_saw would otherwise emit `llvm_alias "std::tuple<…>"`
            // which doesn't exist in the loaded bitcode.
            TypeInfo::Opaque { size_bytes: 0, .. } => "llvm_array 16 (llvm_int 8)".to_string(),
            // Unsized struct: same fallback.
            TypeInfo::Struct { size_bytes: None, .. } => "llvm_array 16 (llvm_int 8)".to_string(),
            other => type_to_saw(other),
        }
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

    out.push_str(&setup);

    // Execute
    out.push_str(&format!(
        "\n    llvm_execute_func [{}];\n",
        args.join(", "),
    ));

    // Return value
    if let Some(saw_type) = &sret_saw_type {
        // sret: the function returns void; the result lives in *result_ptr.
        out.push_str("\n    // sret return: solver chooses any final value for *result_ptr\n");
        out.push_str(&format!(
            "    ret <- llvm_fresh_var \"ret\" ({saw_type});\n"
        ));
        out.push_str("    llvm_points_to result_ptr (llvm_term ret);\n");
    } else {
        let ret_saw = type_to_saw(&func.return_type);
        if ret_saw != "// void" {
            out.push_str("\n    // Return: unconstrained (solver chooses any value)\n");
            out.push_str(&format!(
                "    ret <- llvm_fresh_var \"ret\" ({ret_saw});\n"
            ));
            out.push_str("    llvm_return (llvm_term ret);\n");
        }
    }

    // Postconditions
    if !postconds.is_empty() {
        out.push_str("\n    // --- Postconditions ---\n");
        out.push_str(&postconds);
    }

    // Havoc globals: any virtual method could modify any accessible global
    if !globals.is_empty() {
        out.push_str("\n    // --- Havoced globals ---\n");
        out.push_str("    // A virtual method could be ANY implementation, including one\n");
        out.push_str("    // that writes to global variables. Model as unconstrained.\n");
        for global in globals {
            let safe_name = sanitize_name(&global.name);
            let saw_type = type_to_saw(&global.ty);
            out.push_str(&format!(
                "    {safe_name}_post <- llvm_fresh_var \"{safe_name}_post\" ({saw_type});\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to (llvm_global \"{}\") (llvm_term {safe_name}_post);\n",
                global.mangled_name,
            ));
        }
    }

    out.push_str("};\n");
    out
}

/// Whether a parameter's memory is preserved or havoced.
#[derive(Debug, Clone, Copy, PartialEq)]
enum HavocBehavior {
    /// Memory preserved after call (const / _In_)
    Preserved,
    /// Memory havoced: solver picks any final value (_Out_ / _Inout_ / mutable)
    Havoced,
}

/// Resolve parameter behavior from SAL annotations + type system.
///
/// Resolve parameter behavior: **strictest constraint wins.**
///
/// Sources (any saying "preserved" forces preserved):
///   - Type system: `const T*` / `const T&` → preserved
///   - SAL `_In_` / `_In_reads_` → preserved
///   - SAL `_Out_` / `_Out_writes_` → havoced (unless const overrides)
///   - SAL `_Inout_` → havoced (unless const overrides)
///   - non-const with no annotation → havoced
///
/// If ANY source says "preserved", the result is preserved.
fn resolve_param_behavior(param: &ParamInfo) -> HavocBehavior {
    let type_says_const = param.mutability == Mutability::Readonly;
    let sal_says_readonly = param.annotations.iter().any(|a| matches!(a, Annotation::InReads(_)));

    // If either the type system or SAL says readonly, it's preserved
    if type_says_const || sal_says_readonly {
        return HavocBehavior::Preserved;
    }

    // Otherwise check if SAL explicitly says writable
    let sal_says_writable = param.annotations.iter().any(|a| {
        matches!(a, Annotation::OutWrites(_) | Annotation::Inout)
    });
    if sal_says_writable {
        return HavocBehavior::Havoced;
    }

    // No annotations — fall back to type system
    match param.mutability {
        Mutability::Readonly => HavocBehavior::Preserved,
        Mutability::Mutable | Mutability::WriteOnly => HavocBehavior::Havoced,
    }
}

/// Emit setup + postconditions for a mutable `this` pointer when we have
/// full class layout. Allocates `this` as the LLVM struct type for the
/// class, sets up symbolic pre-state for vptr + each data member, and
/// emits a postcondition that preserves the vptr but writes fresh
/// symbolics over each data member.
///
/// This is what makes a `const` virtual method on a class with a
/// `mutable` data member behave soundly: the spec lets the override
/// modify any member the type system permits, including those reached
/// via `mutable` or `const_cast`.
fn emit_this_full_class_havoc(
    layout: &ClassConstructor,
    setup: &mut String,
    postconds: &mut String,
) {
    setup.push_str("\n    // Parameter: this (mutable → full-class HAVOC)\n");
    setup.push_str(&format!(
        "    this_ptr <- llvm_alloc (llvm_alias \"{}\");\n",
        layout.layout_type_name,
    ));
    // Pre-state: vptr is a pointer (its target is the vtable — we don't
    // care about its contents here, only that it occupies the first
    // slot). Allocate a placeholder pointer so SAW can match against the
    // actual call site's vtable pointer. Each data member gets a fresh
    // symbolic.
    setup.push_str(
        "    this_vptr_pre <- llvm_alloc_readonly_aligned 8 (llvm_int 64);\n",
    );
    for (fname, fty, _) in &layout.layout_fields {
        let width = match fty {
            TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => *w,
            TypeInfo::Bool => 1,
            _ => 64,
        };
        setup.push_str(&format!(
            "    this_{fname}_pre <- llvm_fresh_var \"this_{fname}_pre\" (llvm_int {width});\n",
        ));
    }
    setup.push_str("    llvm_points_to this_ptr (llvm_struct_value\n");
    setup.push_str("        [ this_vptr_pre  // vptr (any pointer)\n");
    for (fname, _, _) in &layout.layout_fields {
        setup.push_str(&format!("        , llvm_term this_{fname}_pre\n"));
    }
    setup.push_str("        ]);\n");

    // Post-state: vptr preserved (same SetupValue), members havoced.
    postconds.push_str("    // this: HAVOCED — data members may have changed\n");
    for (fname, fty, _) in &layout.layout_fields {
        let width = match fty {
            TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => *w,
            TypeInfo::Bool => 1,
            _ => 64,
        };
        postconds.push_str(&format!(
            "    this_{fname}_post <- llvm_fresh_var \"this_{fname}_post\" (llvm_int {width});\n",
        ));
    }
    postconds.push_str("    llvm_points_to this_ptr (llvm_struct_value\n");
    postconds.push_str("        [ this_vptr_pre  // vptr preserved\n");
    for (fname, _, _) in &layout.layout_fields {
        postconds.push_str(&format!("        , llvm_term this_{fname}_post\n"));
    }
    postconds.push_str("        ]);\n");
}

/// Emit setup + postconditions for a pointer parameter using the adversarial model.
///
/// For structs with known fields: decomposes into per-field fresh vars.
/// For structs with only a size: uses opaque byte array.
/// For nested pointers in struct fields: allocates and havocs/preserves targets.
fn emit_adversarial_param(
    name: &str,
    inner_ty: &TypeInfo,
    behavior: HavocBehavior,
    annotations: &[Annotation],
    setup: &mut String,
    postconds: &mut String,
    // When `Some(expr)`, a write-only (`_Out_`) pointer gets the postcondition
    // `*ptr = {{ <expr> }}` (linking the havoc spec to the user-supplied
    // Cryptol function) instead of an unconstrained fresh symbolic. Has no
    // effect on non-WriteOnly params.
    cryptol_post_expr: Option<&str>,
) -> () {
    let is_preserved = behavior == HavocBehavior::Preserved;
    let is_write_only = annotations
        .iter()
        .any(|a| matches!(a, Annotation::OutWrites(_)));
    let alloc_kind = if is_preserved { "llvm_alloc_readonly" } else { "llvm_alloc" };

    // Determine annotation label for comments
    let ann_label = annotation_label(annotations, is_preserved);

    // Special case: `this` pointer for abstract classes (Opaque with no size).
    // The contents include a vtable pointer which is concrete, not symbolic.
    // We allocate without constraining contents so SAW can match any pointer value.
    if name == "this" && matches!(inner_ty, TypeInfo::Opaque { size_bytes: 0, .. }) {
        let saw_type = type_to_saw(inner_ty);
        setup.push_str(&format!("\n    // Parameter: {name} ({ann_label})\n"));
        setup.push_str(&format!("    {name}_ptr <- {alloc_kind} ({saw_type});\n"));
        // No llvm_points_to — contents may be a concrete vtable pointer
        return;
    }

    // Check for sized buffer annotations: _In_reads_(n) or _Out_writes_(n)
    let buffer_size = annotations.iter().find_map(|a| match a {
        Annotation::InReads(n) if *n > 0 => Some(*n),
        Annotation::OutWrites(n) if *n > 0 => Some(*n),
        _ => None,
    });

    if let Some(n) = buffer_size {
        // SAL-annotated buffer: allocate array of exact size
        let elem_saw = match inner_ty {
            TypeInfo::Pointer(elem) => type_to_saw(elem),
            _ => type_to_saw(inner_ty),
        };
        let buf_type = format!("llvm_array {n} ({elem_saw})");

        setup.push_str(&format!("\n    // Parameter: {name} ({ann_label}, buffer[{n}])\n"));
        setup.push_str(&format!("    {name}_ptr <- {alloc_kind} ({buf_type});\n"));
        setup.push_str(&format!(
            "    {name}_val <- llvm_fresh_var \"{name}\" ({buf_type});\n"
        ));
        setup.push_str(&format!("    llvm_points_to {name}_ptr (llvm_term {name}_val);\n"));

        if is_preserved {
            postconds.push_str(&format!("    // {name}: _In_reads_({n}) → buffer preserved\n"));
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
        return;
    }

    // Struct with known fields → decompose per-field
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

    // Default: opaque type (no field info)
    let saw_type = type_to_saw(inner_ty);
    setup.push_str(&format!("\n    // Parameter: {name} ({ann_label})\n"));
    setup.push_str(&format!("    {name}_ptr <- {alloc_kind} ({saw_type});\n"));

    if is_preserved {
        // Preserved: bind a fresh var to the pre-state value and re-assert
        // it after the call. The precondition `llvm_points_to` is necessary
        // for the override to capture the caller's value (which the
        // postcondition then preserves).
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
    } else {
        // Havoced: do NOT add a `llvm_points_to` precondition. The caller
        // may pass uninitialized memory (e.g. an `_Out_` buffer); requiring
        // a readable pre-state value would make the override unmatchable
        // against an uninitialized stack allocation.
        if let (true, Some(expr)) = (is_write_only, cryptol_post_expr) {
            // Tier-2 link: this `_Out_` buffer is the result of the
            // user-supplied Cryptol function applied to the input scalars.
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
}

/// Emit struct with per-field decomposition.
fn emit_struct_decomposed(
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
    setup.push_str(&format!("    {param_name}_ptr <- {alloc_kind} ({saw_type});\n"));

    // Pre-state: allocate fresh var per scalar field, alloc per pointer field
    let mut pre_field_terms = Vec::new();
    let mut nested_ptrs: Vec<(String, String, bool)> = Vec::new(); // (name, saw_type, is_ptr)

    for (field_name, field_ty) in fields {
        let var_name = format!("{param_name}_{field_name}");
        match field_ty {
            TypeInfo::Pointer(pointee) => {
                let pointee_saw = type_to_saw(pointee);
                let nested_alloc = if is_preserved { "llvm_alloc_readonly" } else { "llvm_alloc" };
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

    // Wire struct pre-state
    setup.push_str(&format!(
        "    llvm_points_to {param_name}_ptr (llvm_struct_value\n        [ {} ]);\n",
        pre_field_terms.join("\n        , "),
    ));

    // Post-state
    if is_preserved {
        // All fields preserved
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
        // All fields havoced
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
                // The pointer value in the struct itself is also havoced
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

/// Generate a human-readable label for the annotation-derived behavior.
fn annotation_label(annotations: &[Annotation], is_preserved: bool) -> String {
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

/// Generate the index file with vtable-aware override setup.
///
/// Produces:
/// - Includes for all havoc spec files
/// - llvm_unsafe_assume_spec for each stub
/// - Per-class helper: `alloc_<class>_this` that allocates a `this` pointer
///   whose first 8 bytes (vptr) point to the concrete vtable global
fn generate_override_index_with_vtable(
    methods: &[InterfaceMethod],
    by_class: &std::collections::BTreeMap<String, Vec<&InterfaceMethod>>,
    constructors: &[ClassConstructor],
    classes_with_vdtor: &std::collections::HashSet<String>,
) -> String {
    let mut out = String::new();
    out.push_str("// Auto-generated interface override setup with vtable resolution\n");
    out.push_str("//\n");
    out.push_str("// For DIRECT calls (function is 'declare' in bitcode):\n");
    out.push_str("//   Just use llvm_unsafe_assume_spec with the mangled name directly.\n");
    out.push_str("//   No extra files needed — the havoc specs bind to the declared symbol.\n");
    out.push_str("//\n");
    out.push_str("// For VTABLE INDIRECT calls (virtual dispatch through this->vptr):\n");
    out.push_str("//   m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("//   m       <- llvm_combine_modules m_main m_stubs;\n");
    out.push_str("//   The vtable global provides the function pointer resolution chain:\n");
    out.push_str("//     this->vptr -> vtable[slot] -> stub function -> havoc spec\n");
    out.push_str("//\n");
    out.push_str("// Usage:\n");
    out.push_str("//   m_main  <- llvm_load_module \"your_code.bc\";\n");
    out.push_str("//   m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("//   m       <- llvm_combine_modules m_main m_stubs;\n");
    out.push_str("//   include \"interface_overrides.saw\";\n");
    out.push_str("//\n");
    out.push_str("// For each interface class, a helper is generated:\n");
    out.push_str("//   alloc_<class>_this : LLVMSetup Term\n");
    out.push_str("// which allocates a properly-formed `this` pointer with vptr wired\n");
    out.push_str("// to the stub vtable. Use it in your verification specs.\n\n");

    // Include each havoc spec file — overrides reuse the originating
    // class's spec (and don't have their own file), so skip them.
    for method in methods {
        if method.is_override {
            continue;
        }
        let filename = format!(
            "{}_{}_havoc_spec.saw",
            sanitize_name(&method.class_name),
            sanitize_name(&method.method.name),
        );
        out.push_str(&format!("include \"{filename}\";\n"));
    }
    out.push('\n');

    // Emit llvm_unsafe_assume_spec for each method
    let mut all_overrides = Vec::new();

    out.push_str("// --- Override bindings ---\n");
    out.push_str("// For vtable dispatch: binds to stub function names (from vtable_stubs.ll)\n");
    out.push_str("// For direct calls: binds to the mangled C++ name (already in your bitcode)\n\n");

    for method in methods {
        // Overrides share their originating class's stub function — emitting
        // a separate `llvm_unsafe_assume_spec` here would reference a stub
        // symbol that does not exist in vtable_stubs.ll, and would also try
        // to redefine the (already-asserted) base override.
        if method.is_override {
            continue;
        }
        let stub_name = stub_function_name(method);
        let safe_name = sanitize_name(&stub_name);
        let mangled = method
            .method
            .mangled_name
            .as_deref();

        out.push_str(&format!(
            "// {}::{} — havoc: any behavior consistent with type constraints\n",
            method.class_name, method.method.name,
        ));

        // Vtable stub binding (for indirect calls via vtable_stubs.ll)
        out.push_str(&format!(
            "ov_{safe_name} <- llvm_unsafe_assume_spec m \"{stub_name}\" {safe_name}_havoc;\n",
        ));
        all_overrides.push(format!("ov_{safe_name}"));

        // Mangled name binding (documentation only)
        // When the compiler devirtualizes a call, SAW will execute the real
        // method body directly. That's typically better than havoc since it
        // gives a real proof of the concrete implementation. We do NOT bind
        // an override to the mangled name — let the real symbol be verified.
        if let Some(mangled_name) = mangled {
            let safe_mangled = sanitize_name(mangled_name);
            if method.is_pure {
                out.push_str(
                    "// Pure virtual — no real symbol in bitcode.\n"
                );
            } else {
                out.push_str(
                    "// Devirtualized direct calls hit the real symbol — no override.\n"
                );
            }
            out.push_str(&format!(
                "// To force havoc instead, uncomment:\n"
            ));
            out.push_str(&format!(
                "// ov_{safe_mangled} <- llvm_unsafe_assume_spec m \"{mangled_name}\" {safe_name}_havoc;\n",
            ));
        }
        out.push('\n');
    }

    // Emit per-class vtable setup helpers
    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();

        out.push_str(&format!(
            "// ===========================================================\n"
        ));
        out.push_str(&format!(
            "// {class_name}: vtable setup helper\n"
        ));
        out.push_str(&format!(
            "// ===========================================================\n"
        ));
        out.push_str(&format!(
            "//\n"
        ));
        out.push_str(&format!(
            "// Allocates a `this` pointer (8 bytes for vptr) with the vtable\n"
        ));
        out.push_str(&format!(
            "// pointer wired to {safe_class}_vtable. SAW resolves:\n"
        ));
        out.push_str(&format!(
            "//   this -> vptr -> {safe_class}_vtable[slot] -> stub -> havoc spec\n"
        ));
        out.push_str(&format!(
            "//\n"
        ));
        out.push_str(&format!(
            "// Use in your spec like:\n"
        ));
        out.push_str(&format!(
            "//   this_ptr <- alloc_{safe_class}_this;\n"
        ));

        // Collect override names for this class
        let class_ov_names: Vec<String> = class_methods
            .iter()
            .map(|m| format!("ov_{}", sanitize_name(&stub_function_name(m))))
            .collect();
        if !class_ov_names.is_empty() {
            out.push_str(&format!(
                "//   llvm_verify m \"target\" [{}] false spec z3;\n",
                class_ov_names.join(", "),
            ));
        }
        out.push('\n');

        // MSVC ABI: a single deleting-destructor slot when the class has
        // a virtual dtor; matches the layout emitted in `vtable_stubs.ll`.
        let dtor_slots = if classes_with_vdtor.contains(class_name.as_str()) { 1 } else { 0 };
        let slot_count = class_methods.len() + dtor_slots;

        out.push_str(&format!(
            "let alloc_{safe_class}_this : LLVMSetup SetupValue = do {{\n"
        ));
        out.push_str(&format!(
            "    // Allocate the vtable: {slot_count} function pointers\n"
        ));
        out.push_str(&format!(
            "    let vtable_ty = llvm_array {slot_count} (llvm_int 64);\n"
        ));
        out.push_str(&format!(
            "    vtable_ptr <- llvm_alloc_readonly_aligned 8 vtable_ty;\n"
        ));
        out.push_str(&format!(
            "\n    // Point vtable slots to the global stub vtable\n"
        ));
        out.push_str(&format!(
            "    llvm_points_to_at_type vtable_ptr vtable_ty\n"
        ));
        out.push_str(&format!(
            "        (llvm_global_initializer \"{safe_class}_vtable\");\n"
        ));
        out.push_str(
            "\n    // Allocate `this`: first 8 bytes = vptr -> vtable\n"
        );
        out.push_str(
            "    this_ptr <- llvm_alloc (llvm_int 64);\n"
        );
        out.push_str(
            "    llvm_points_to this_ptr vtable_ptr;\n"
        );
        out.push_str(&format!(
            "\n    return this_ptr;\n"
        ));
        out.push_str("};\n\n");
    }

    // Emit convenience: list of all override names
    out.push_str("// All overrides for use in llvm_verify:\n");
    out.push_str(&format!(
        "// let all_interface_overrides = [{}];\n",
        all_overrides.join(", "),
    ));

    // ===========================================================
    // Constructor override specs
    // ===========================================================
    //
    // For code that constructs objects internally (e.g. new OkLog()),
    // SAW can't resolve the vtable dispatch because different constructors
    // write different vtable pointers, creating a symbolic merge.
    //
    // These overrides replace the real constructors: after the override,
    // this->vptr points to the FIRST base class's stub vtable. This way
    // all branches resolve to the same stub function → single havoc spec.
    if !constructors.is_empty() {
        out.push_str("\n// ===========================================================\n");
        out.push_str("// Constructor overrides for vtable wiring\n");
        out.push_str("// ===========================================================\n");
        out.push_str("//\n");
        out.push_str("// These override real constructors to wire vptr to stub vtables.\n");
        out.push_str("// Use when verifying functions that construct objects internally\n");
        out.push_str("// (e.g. `new OkLog()`) rather than receiving them as parameters.\n\n");

        for ctor in constructors {
            let safe_class = sanitize_name(&ctor.class_name).to_lowercase();

            // Every class we have a constructor for should also have stub
            // vtables generated — clang exposes overriding methods via
            // `OverrideAttr`, so derived classes appear in `by_class` too.
            // If a class somehow has no stubs (e.g. an empty class with no
            // virtual methods at all), skip it: there's nothing to wire to.
            if !by_class.contains_key(&ctor.class_name) {
                continue;
            }
            let target_vtable = safe_class.clone();

            let slot_count = by_class
                .get(&ctor.class_name)
                .map(|ms| {
                    // MSVC ABI: single deleting-dtor slot, not the
                    // Itanium pair of complete + deleting dtors.
                    let dtor_slots = if classes_with_vdtor.contains(&ctor.class_name) {
                        1
                    } else {
                        0
                    };
                    ms.len() + dtor_slots
                })
                .unwrap_or(1);

            out.push_str(&format!(
                "// Constructor override for {}: wires vptr to {}_vtable\n",
                ctor.class_name, target_vtable,
            ));
            out.push_str(&format!(
                "let {safe_class}_ctor_override : LLVMSetup () = do {{\n"
            ));

            // Pick allocation type. When the class has any non-vptr field,
            // we must allocate the full class layout (a named LLVM struct
            // type) AND initialize each field — otherwise reads past the
            // vptr return fresh symbolic values and produce phantom
            // counterexamples. For interfaces with only the vptr, the
            // legacy 8-byte i64 allocation is sufficient.
            let has_data_members = !ctor.layout_fields.is_empty();
            if has_data_members {
                out.push_str(
                    "\n    // this: full class layout (vptr + data members)\n",
                );
                out.push_str(&format!(
                    "    this_ptr <- llvm_alloc (llvm_alias \"{}\");\n",
                    ctor.layout_type_name,
                ));
            } else {
                out.push_str(
                    "\n    // this: passed in from operator new (precondition)\n",
                );
                out.push_str(
                    "    this_ptr <- llvm_alloc_aligned 8 (llvm_int 64);\n",
                );
            }
            out.push_str("\n    llvm_execute_func [this_ptr];\n");
            out.push_str(
                "\n    // Postcondition: allocate stub vtable and wire vptr\n",
            );
            out.push_str(&format!(
                "    let vtable_ty = llvm_array {slot_count} (llvm_int 64);\n"
            ));
            out.push_str(
                "    vtable_ptr <- llvm_alloc_readonly_aligned 8 vtable_ty;\n",
            );
            out.push_str("    llvm_points_to_at_type vtable_ptr vtable_ty\n");
            out.push_str(&format!(
                "        (llvm_global_initializer \"{target_vtable}_vtable\");\n",
            ));
            if has_data_members {
                // Initialize all members in one write via llvm_struct_value:
                //   [vptr, field0, field1, ...]
                // Each field uses its in-class initializer (or 0 for POD
                // types without one). This matches what the real
                // constructor would do, so reads after construction see
                // concrete values instead of fresh symbolics.
                out.push_str("    // Initialize vptr + all data members\n");
                out.push_str("    llvm_points_to this_ptr (llvm_struct_value\n");
                out.push_str("        [ vtable_ptr\n");
                for (fname, fty, default_lit) in &ctor.layout_fields {
                    let width = match fty {
                        TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => *w,
                        TypeInfo::Bool => 1,
                        // Non-integer fields (pointers, nested structs)
                        // aren't supported by this initializer path yet.
                        // Default to a 64-bit zero so the struct value is
                        // still well-typed; over-approximation, not under.
                        _ => 64,
                    };
                    // C++ `T t;` default-initializes POD members to
                    // indeterminate values, but `new T()` value-initializes
                    // them to 0. In the absence of an explicit in-class
                    // initializer we assume value-init (the common case
                    // when `new` is used), which is what the original
                    // constructor would have produced.
                    let lit = if default_lit.is_empty() {
                        "0"
                    } else {
                        default_lit.as_str()
                    };
                    out.push_str(&format!(
                        "        , llvm_term {{{{ {lit} : [{width}] }}}}  // {fname}\n",
                    ));
                }
                out.push_str("        ]);\n");
            } else {
                out.push_str("    llvm_points_to this_ptr vtable_ptr;\n");
            }
            out.push_str("    llvm_return this_ptr;\n");
            out.push_str("};\n");
            out.push_str(&format!(
                "ov_{safe_class}_ctor <- llvm_unsafe_assume_spec m \"{}\" {safe_class}_ctor_override;\n\n",
                ctor.mangled_name,
            ));

            all_overrides.push(format!("ov_{safe_class}_ctor"));
        }
    }

    out
}

/// Description of a single `this`-class field for `emit_container_this`.
enum FieldKind {
    /// Pointer to an interface — allocate via `alloc_<iface>_this`.
    Interface(String, String), // (field_name, interface_class_name)
    /// Anything else — allocate a fresh 8-byte slot so reads at this
    /// offset return a deterministic value rather than tripping
    /// "outside of the allocation".
    Other(String), // field_name
}

/// A container class is a non-interface class whose own fields include
/// at least one pointer to an interface in `interface_classes`.
/// Recognises both raw pointers (`IFoo*` → `Pointer(Struct/Opaque{name:"IFoo"})`)
/// and the common smart-pointer wrappers (`std::unique_ptr<IFoo>` /
/// `std::shared_ptr<IFoo>` whose `TypeInfo::Struct` carries a templated
/// name like `"std::unique_ptr<IFoo>"`). Returns `None` when no field
/// matches — caller should fall back to the standard `llvm_alloc` path.
fn container_layout_for(
    ty: &TypeInfo,
    interface_classes: &std::collections::HashSet<String>,
) -> Option<Vec<FieldKind>> {
    let TypeInfo::Pointer(inner) = ty else {
        return None;
    };
    let TypeInfo::Struct { name, fields, .. } = inner.as_ref() else {
        return None;
    };
    if fields.is_empty() || interface_classes.contains(name) {
        return None;
    }
    let kinds: Vec<FieldKind> = fields
        .iter()
        .map(|(fname, fty)| match field_interface_name(fty, interface_classes) {
            Some(iface) => FieldKind::Interface(fname.clone(), iface),
            None => FieldKind::Other(fname.clone()),
        })
        .collect();
    if kinds
        .iter()
        .any(|k| matches!(k, FieldKind::Interface(..)))
    {
        Some(kinds)
    } else {
        None
    }
}

/// Extract the interface class name from a field type. Recognises raw
/// pointers and the standard smart-pointer wrappers whose templated
/// type name embeds the pointee (`std::unique_ptr<IFoo>`,
/// `std::shared_ptr<IFoo>`).
fn field_interface_name(
    ty: &TypeInfo,
    interface_classes: &std::collections::HashSet<String>,
) -> Option<String> {
    // Raw pointer to interface.
    if let TypeInfo::Pointer(inner) = ty {
        let pointee = match inner.as_ref() {
            TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.as_str()),
            _ => None,
        };
        if let Some(n) = pointee {
            if interface_classes.contains(n) {
                return Some(n.to_string());
            }
        }
    }
    // Smart-pointer wrapper.
    let wrapped = match ty {
        TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.as_str()),
        _ => None,
    };
    if let Some(name) = wrapped {
        for wrap in ["std::unique_ptr<", "unique_ptr<", "std::shared_ptr<", "shared_ptr<"] {
            if let Some(rest) = name.strip_prefix(wrap) {
                if let Some(end) = rest.find('>') {
                    let inner = rest[..end].trim().trim_end_matches('*').trim();
                    let simple = inner.split_whitespace().next().unwrap_or(inner);
                    if interface_classes.contains(simple) {
                        return Some(simple.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Emit a SAW block that allocates `<param>_ptr` as a packed struct whose
/// interface-typed slots point to objects wired to their stub vtables,
/// matching the layout the bitcode reads through `getelementptr`. Slots
/// for non-interface fields get fresh 8-byte allocations so any load at
/// their offset returns a defined symbolic value.
fn emit_container_this(out: &mut String, param_name: &str, fields: &[FieldKind]) {
    out.push_str(&format!(
        "    // {param_name}: container class with interface fields — wire each vptr.\n",
    ));
    let mut slot_values: Vec<String> = Vec::with_capacity(fields.len());
    for (i, kind) in fields.iter().enumerate() {
        let slot_name = format!("{param_name}_f{i}");
        match kind {
            FieldKind::Interface(fname, iface) => {
                let safe_iface = sanitize_name(iface).to_lowercase();
                out.push_str(&format!(
                    "    {slot_name} <- alloc_{safe_iface}_this;  // {fname} : {iface}*\n",
                ));
                slot_values.push(slot_name);
            }
            FieldKind::Other(fname) => {
                out.push_str(&format!(
                    "    {slot_name} <- llvm_alloc (llvm_int 64);  // {fname} (opaque slot)\n",
                ));
                slot_values.push(slot_name);
            }
        }
    }
    let n = fields.len();
    out.push_str(&format!(
        "    {param_name}_ptr <- llvm_alloc (llvm_array {n} (llvm_int 64));\n",
    ));
    out.push_str(&format!(
        "    llvm_points_to {param_name}_ptr (llvm_packed_struct_value [{}]);\n",
        slot_values.join(", "),
    ));
}

/// Emit a complete, runnable SAW verification script that checks a C++ function
/// against a Cryptol spec.
///
/// Generates `verify.saw` in `output_dir` that:
/// 1. Loads bitcode + vtable stubs
/// 2. Assumes external/virtual function specs
/// 3. Imports the Cryptol spec
/// 4. Calls llvm_verify to check equivalence
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

    // Set of interface class names — used to detect interface-pointer boundaries
    let interface_classes: std::collections::HashSet<String> = vmethods
        .iter()
        .map(|m| m.class_name.clone())
        .collect();

    // Helper: if a type is a pointer to an interface class, return the name.
    // Abstract classes (only virtual methods, no fields) parse as Opaque rather
    // than Struct, so we accept both. Single rule that drives boundary handling
    // everywhere — parameters, returns, fields, globals.
    let interface_of = |ty: &TypeInfo| -> Option<String> {
        if let TypeInfo::Pointer(inner) = ty {
            let name_opt = match inner.as_ref() {
                TypeInfo::Struct { name, .. } => Some(name.as_str()),
                TypeInfo::Opaque { name, .. } => Some(name.as_str()),
                _ => None,
            };
            if let Some(name) = name_opt {
                if interface_classes.contains(name) {
                    return Some(name.to_string());
                }
            }
        }
        None
    };

    // Build relative paths from verify.saw's location
    let bitcode_rel = pathdiff_or_absolute(bitcode_path, output_dir);
    let cryptol_rel = pathdiff_or_absolute(cryptol_spec_path, output_dir);

    // Detect which externals need special handling
    let needs_operator_new = external_calls.iter().any(|s| {
        s.function_name == "operator new" || s.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z")
    });
    let needs_experimental =
        !external_calls.is_empty() || has_interfaces || !sub_callee_specs.is_empty();

    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "// Auto-generated SAW verification script\n\
         // Function: {} ({})\n\
         // Cryptol spec: {} :: {}\n\
         // Generated by saw-spec-gen gen-verify\n\n",
        function_name, mangled_name, cryptol_rel, cryptol_fn,
    ));

    if needs_experimental {
        out.push_str("enable_experimental;\n\n");
    }

    // Step 1: Load bitcode
    let mut step = 1;
    out.push_str(&format!("// Step {step}: Load bitcode"));
    if has_interfaces {
        // Always reference `vtable_stubs.bc` in the script — SAW's
        // `llvm_load_module` only accepts bitcode, and `script_filename`
        // returns the `.bc` name even when auto-assembly failed. When
        // auto-assembly failed we emit a warning above the load so the
        // user knows to assemble the `.ll` to `.bc` before running SAW.
        let stubs_filename = stubs_status
            .script_filename()
            .unwrap_or("vtable_stubs.bc");
        out.push_str(" + vtable stubs\n");
        if let AssembledStubs::TextOnly { ll_filename } = stubs_status {
            out.push_str("//\n");
            out.push_str(
                "// WARNING: vtable_stubs.bc was NOT auto-assembled at gen time\n",
            );
            out.push_str(
                "//          (llvm-as / clang not found on PATH). Assemble it before\n",
            );
            out.push_str("//          running SAW:\n");
            out.push_str(&format!(
                "//            llvm-as {ll_filename} -o vtable_stubs.bc\n",
            ));
            out.push_str(&format!(
                "//          (or)  clang -c -emit-llvm {ll_filename} -o vtable_stubs.bc\n",
            ));
            out.push_str(
                "//          The load line below already references vtable_stubs.bc.\n",
            );
        } else if let AssembledStubs::Bitcode { assembler, .. } = stubs_status {
            out.push_str(&format!(
                "// vtable_stubs.bc was assembled from vtable_stubs.ll via `{assembler}`.\n",
            ));
        }
        out.push_str(&format!(
            "m_main  <- llvm_load_module \"{}\";\n",
            bitcode_rel,
        ));
        out.push_str(&format!(
            "m_stubs <- llvm_load_module \"{stubs_filename}\";\n",
        ));
        out.push_str("m       <- llvm_combine_modules m_main [m_stubs];\n\n");
    } else {
        out.push_str("\n");
        out.push_str(&format!(
            "m <- llvm_load_module \"{}\";\n\n",
            bitcode_rel,
        ));
    }

    // Step 2: Include interface overrides (must come BEFORE external specs so
    // factory specs for functions returning Interface* can use alloc_<iface>_this).
    // The Cryptol spec MUST be imported here too — the generated havoc specs
    // may reference its functions inside `let X = do { ... {{ cryptol_fn x }} ... }`
    // bindings, which SAW evaluates eagerly at let-bind time inside the include.
    if has_interfaces {
        step += 1;
        out.push_str(&format!("// Step {step}: Vtable havoc overrides + constructor overrides\n"));
        out.push_str(&format!("import \"{}\";\n", cryptol_rel));
        out.push_str("include \"interface_overrides.saw\";\n\n");
    }

    // Step 3: Assume external functions (only those actually called)
    let mut override_names: Vec<String> = Vec::new();

    if !external_calls.is_empty() || needs_operator_new {
        step += 1;
        out.push_str(&format!("// Step {step}: Assume external functions\n"));

        // Emit experimental specs (e.g. rand, printf) via llvm_unspecified_globals
        let mut included: std::collections::HashSet<String> = std::collections::HashSet::new();
        for spec in external_calls {
            // operator new gets a manual spec (pointer alloc), skip from experimental
            if spec.function_name == "operator new" || spec.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z") {
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

        // operator new override (loaded from generated spec file)
        if needs_operator_new {
            out.push_str("include \"specs_experimental/operator_new_auto_spec.saw\";\n");
            override_names.push("ov_new".to_string());
        }
    }

    // Step N: Compositional sub-function overrides.
    //
    // Each in-module function the target directly calls is replaced by an
    // adversarial (havoc) override.  This lets SAW reason locally about the
    // target without descending into complex sub-bodies.  The override return
    // values are fresh symbolic — the user is responsible for wiring them
    // into the Cryptol equivalence check (see TODO in the equivalence spec
    // below).
    if !sub_callee_specs.is_empty() {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Sub-function adversarial overrides (compositional)\n",
        ));
        let mut sub_included: std::collections::HashSet<String> =
            std::collections::HashSet::new();
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

    // Step N: Import Cryptol spec (only if not already imported above for
    // interface havoc specs — re-importing is harmless but noisy).
    if !has_interfaces {
        step += 1;
        out.push_str(&format!(
            "// Step {step}: Import Cryptol spec\n",
        ));
        out.push_str(&format!("import \"{}\";\n\n", cryptol_rel));
    }

    // Step N: Build the equivalence spec
    step += 1;
    out.push_str(&format!(
        "// Step {step}: Equivalence spec — C++ {} == Cryptol {}\n",
        function_name, cryptol_fn,
    ));
    out.push_str(&format!(
        "let {function_name}_equiv_spec = do {{\n"
    ));

    // Parameters — use the target spec's param info, but consult target_fn.params
    // for the real type (so we can detect interface-pointer boundaries).
    let mut cryptol_args = Vec::new();
    let mut execute_args = Vec::new();
    for (i, param) in target_spec.params.iter().enumerate() {
        // Detect interface-pointer parameters: wire vptr to stub vtable instead
        // of plain llvm_alloc, so virtual calls through the param resolve to
        // havoc specs. Interface pointers are NOT passed to the Cryptol spec
        // (they're a runtime concern, not a mathematical one).
        let iface = target_fn
            .params
            .get(i)
            .and_then(|p| interface_of(&p.ty));
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

        // Detect container classes: a pointer to a class whose fields
        // include interface pointers (the common shape for `this` on a
        // method like `MetadataSecurityProtocol::AttestKeyOwnership`
        // that internally invokes virtual methods through stored
        // interface fields). A flat `llvm_alloc (llvm_int 64)` is not
        // enough — the bitcode reads each field via `getelementptr`,
        // which trips an "outside of the allocation" UB at the first
        // non-zero offset. Emit a packed-struct `this` whose interface
        // slots point to objects already wired to their stub vtables.
        if let Some(container) = target_fn
            .params
            .get(i)
            .and_then(|p| container_layout_for(&p.ty, &interface_classes))
        {
            emit_container_this(&mut out, &param.name, &container);
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

    // Global variables
    for global in &target_spec.referenced_globals {
        out.push_str(&format!(
            "    llvm_alloc_global \"{}\";\n",
            global.mangled_name,
        ));
        if let Some(ref init) = global.init_value {
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

    // Execute (uses execute_args built during parameter setup above so that
    // interface-pointer params pass the wired pointer instead of plain alloc)
    out.push_str(&format!(
        "    llvm_execute_func [{}];\n\n",
        execute_args.join(", "),
    ));

    // Postcondition: return == Cryptol spec
    let cryptol_call = if cryptol_args.is_empty() {
        cryptol_fn.to_string()
    } else {
        format!("{} {}", cryptol_fn, cryptol_args.join(" "))
    };
    if sub_callee_specs.is_empty() {
        out.push_str(
            "    // Postcondition: C++ result == Cryptol spec\n",
        );
        out.push_str(&format!(
            "    llvm_return (llvm_term {{{{ {} }}}});\n",
            cryptol_call,
        ));
    } else {
        // Sub-function overrides return fresh symbolic values that may flow
        // into the result.  The mapping between those values and Cryptol spec
        // arguments is application-specific — the tool can't infer it — so
        // emit a TODO skeleton instead of a (likely wrong) auto-derived
        // postcondition.
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

    // Step N: Collect overrides and verify
    step += 1;
    out.push_str(&format!(
        "// Step {step}: Verify equivalence\n",
    ));
    out.push_str(&format!(
        "print \"=== Checking: {} == {} (Cryptol) ===\";\n",
        function_name, cryptol_fn,
    ));

    // Build override list from external calls + interface overrides
    let mut overrides = override_names.clone();
    if has_interfaces {
        // Add constructor overrides — only for classes that actually have constructors
        let ctor_classes: std::collections::HashSet<&str> = constructors
            .iter()
            .map(|c| c.class_name.as_str())
            .collect();
        for class in &ctor_classes {
            let safe_class = sanitize_name(class).to_lowercase();
            overrides.push(format!("ov_{safe_class}_ctor"));
        }
        // Add method stub overrides — only for originating methods.
        // Overrides share their base's stub function and override variable,
        // so referencing a separate `ov_<derived>_<method>_stub` would be
        // an undefined identifier.
        for method in vmethods {
            if method.is_override {
                continue;
            }
            let stub_name = stub_function_name(method);
            let safe_name = sanitize_name(&stub_name);
            overrides.push(format!("ov_{safe_name}"));
        }
    }

    // Format overrides list nicely
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
        "print \"=== VERIFIED: {} == {} ===\";\n",
        function_name, cryptol_fn,
    ));

    // Write the script
    let script_path = output_dir.join("verify.saw");
    fs::write(&script_path, &out)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;

    Ok(())
}

/// Compute a relative path from `base` to `target`, or fall back to the absolute path.
fn pathdiff_or_absolute(target: &Path, base: &Path) -> String {
    // Try to make target relative to base
    if let Ok(target_abs) = target.canonicalize() {
        if let Ok(base_abs) = base.canonicalize() {
            if let Some(rel) = pathdiff(&target_abs, &base_abs) {
                return rel.to_string_lossy().replace('\\', "/");
            }
        }
    }
    // Fall back: if target is already relative, use as-is
    target.to_string_lossy().replace('\\', "/").to_string()
}

/// Simple relative path computation (like the `pathdiff` crate).
fn pathdiff(path: &Path, base: &Path) -> Option<std::path::PathBuf> {
    let mut path_components = path.components().peekable();
    let mut base_components = base.components().peekable();

    // Skip common prefix
    while let (Some(p), Some(b)) = (path_components.peek(), base_components.peek()) {
        if p == b {
            path_components.next();
            base_components.next();
        } else {
            break;
        }
    }

    let mut result = std::path::PathBuf::new();
    for _ in base_components {
        result.push("..");
    }
    for c in path_components {
        result.push(c);
    }
    Some(result)
}

/// Generate the extern "C" stub function name.
fn stub_function_name(method: &InterfaceMethod) -> String {
    format!(
        "{}_{}_stub",
        sanitize_name(&method.class_name).to_lowercase(),
        sanitize_name(&method.method.name).to_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(
        name: &str,
        params: Vec<ParamConstraint>,
        return_type: &str,
        can_throw: bool,
    ) -> SpecConstraint {
        SpecConstraint {
            function_name: name.into(),
            mangled_name: None,
            params,
            return_constraint: ReturnConstraint {
                saw_type: return_type.into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
            },
            can_throw,
            is_virtual: false,
            has_body: true,
            referenced_globals: vec![],
            postconditions: vec![],
        }
    }

    #[test]
    fn test_generate_llvm_spec_readonly() {
        let spec = make_spec(
            "test_fn",
            vec![ParamConstraint {
                name: "x".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            "// void",
            false,
        );
        let output = generate_saw_spec(&spec);
        assert!(output.contains("LLVMSetup ()"));
        assert!(output.contains("llvm_alloc_readonly"));
        assert!(output.contains("llvm_fresh_var"));
        assert!(output.contains("llvm_execute_func"));
    }

    #[test]
    fn test_generate_llvm_spec_mutable() {
        let spec = make_spec(
            "mutate_fn",
            vec![ParamConstraint {
                name: "buf".into(),
                alloc_type: AllocType::AllocMutable,
                saw_type: "llvm_int 64".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            }],
            "// void",
            false,
        );
        let output = generate_saw_spec(&spec);
        assert!(output.contains("llvm_alloc (llvm_int 64)"));
        assert!(!output.contains("llvm_alloc_readonly"));
    }

    #[test]
    fn test_generate_llvm_spec_freshvar() {
        let spec = make_spec(
            "add",
            vec![
                ParamConstraint {
                    name: "a".into(),
                    alloc_type: AllocType::FreshVar,
                    saw_type: "llvm_int 32".into(),
                    preconditions: vec![],
                    unchanged_after: false,
                    dereferenceable_size: None,
                },
                ParamConstraint {
                    name: "b".into(),
                    alloc_type: AllocType::FreshVar,
                    saw_type: "llvm_int 32".into(),
                    preconditions: vec![],
                    unchanged_after: false,
                    dereferenceable_size: None,
                },
            ],
            "llvm_int 32",
            false,
        );
        let output = generate_saw_spec(&spec);
        assert!(output.contains("a <- llvm_fresh_var \"a\" (llvm_int 32)"));
        assert!(output.contains("llvm_term a, llvm_term b"));
    }

    #[test]
    fn test_generate_llvm_spec_return() {
        let spec = make_spec(
            "get_val",
            vec![],
            "llvm_int 32",
            false,
        );
        let output = generate_saw_spec(&spec);
        // Concrete function: unconstrained return for immediate use with llvm_verify
        assert!(output.contains("ret <- llvm_fresh_var \"ret\" (llvm_int 32)"));
        assert!(output.contains("llvm_return (llvm_term ret)"));
        assert!(output.contains("ACTION REQUIRED"));
        assert!(output.contains("llvm_verify"));
        assert!(!output.contains("llvm_unsafe_assume_spec"));
    }

    #[test]
    fn test_generate_llvm_spec_void_return() {
        let spec = make_spec("noop", vec![], "// void", false);
        let output = generate_saw_spec(&spec);
        assert!(!output.contains("llvm_return"));
    }

    #[test]
    fn test_generate_llvm_spec_can_throw() {
        let spec = make_spec("risky", vec![], "// void", true);
        let output = generate_saw_spec(&spec);
        assert!(output.contains("WARNING: Function may throw"));
    }

    #[test]
    fn test_generate_mir_spec_basic() {
        let spec = make_spec(
            "rust_fn",
            vec![ParamConstraint {
                name: "x".into(),
                alloc_type: AllocType::FreshVar,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
            }],
            "llvm_int 32",
            false,
        );
        let output = generate_mir_spec(&spec);
        assert!(output.contains("MIRSetup ()"));
        assert!(output.contains("mir_fresh_var"));
        assert!(output.contains("mir_execute_func"));
        assert!(output.contains("mir_return"));
    }

    #[test]
    fn test_generate_mir_spec_alloc() {
        let spec = make_spec(
            "rust_ref_fn",
            vec![ParamConstraint {
                name: "data".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 64".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            "// void",
            false,
        );
        let output = generate_mir_spec(&spec);
        assert!(output.contains("mir_alloc_readonly"));
        assert!(output.contains("mir_points_to"));
    }

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("simple"), "simple");
        assert_eq!(sanitize_name("my::func"), "my__func");
        assert_eq!(sanitize_name("ns::Class<int>"), "ns__Class_int_");
    }

    #[test]
    fn test_mir_saw_type_mapping() {
        assert_eq!(mir_saw_type("llvm_int 32"), "mir_u32");
        assert_eq!(mir_saw_type("llvm_int 8"), "mir_u8");
        assert_eq!(
            mir_saw_type("llvm_alias \"MyStruct\""),
            "mir_find_adt m \"MyStruct\""
        );
    }

    #[test]
    fn test_emit_saw_specs_creates_files() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_emit");
        let _ = fs::remove_dir_all(&dir);

        let specs = vec![make_spec("test_fn", vec![], "// void", false)];
        emit_saw_specs(&specs, &dir, false).unwrap();

        assert!(dir.join("test_fn_auto_spec.saw").exists());
        assert!(dir.join("auto_specs.saw").exists());

        let index = fs::read_to_string(dir.join("auto_specs.saw")).unwrap();
        assert!(index.contains("include \"test_fn_auto_spec.saw\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_emit_mir_saw_specs() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_mir_emit");
        let _ = fs::remove_dir_all(&dir);

        let specs = vec![make_spec("rust_fn", vec![], "llvm_int 32", false)];
        emit_mir_saw_specs(&specs, &dir).unwrap();

        let content =
            fs::read_to_string(dir.join("rust_fn_auto_spec.saw")).unwrap();
        assert!(content.contains("MIRSetup"));

        let index = fs::read_to_string(dir.join("auto_specs.saw")).unwrap();
        assert!(index.contains("Mode: MIR"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_postconditions() {
        let spec = SpecConstraint {
            function_name: "read_fn".into(),
            mangled_name: None,
            params: vec![ParamConstraint {
                name: "data".into(),
                alloc_type: AllocType::AllocReadonly,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: true,
                dereferenceable_size: None,
            }],
            return_constraint: ReturnConstraint {
                saw_type: "// void".into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            referenced_globals: vec![],
            postconditions: vec![
                "llvm_points_to data_ptr (llvm_term data_before)".into(),
            ],
        };
        let output = generate_saw_spec(&spec);
        assert!(output.contains("Postconditions"));
        assert!(output.contains("data_before"));
    }

    fn make_iface_method(
        class: &str,
        name: &str,
        ret: TypeInfo,
        offset: u64,
    ) -> InterfaceMethod {
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

    /// Regression: vtable slots must be ordered by source-declaration
    /// offset, not by the order `extract_virtual_methods` happens to walk
    /// the AST. MSVC assigns vtable slots in declaration order, so a
    /// scrambled walk order would silently misroute virtual dispatch.
    #[test]
    fn test_vtable_slot_order_follows_source_offset() {
        // Intentionally pass methods in reverse declaration order to make
        // sure the sort is what fixes the layout — not the input order.
        let methods = vec![
            make_iface_method("IRV", "Authenticate", TypeInfo::Bool, 200),
            make_iface_method("IRV", "IsValidMetadataHeader", TypeInfo::Bool, 100),
        ];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_order");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &std::collections::HashSet::new(),
            &dir,
            None,
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();

        // The vtable global must list IsValidMetadataHeader (offset 100)
        // BEFORE Authenticate (offset 200) — that matches MSVC's
        // declaration-order slot assignment.
        let vtable_section = ll
            .split("@irv_vtable")
            .nth(1)
            .expect("vtable global missing");
        let is_valid_pos = vtable_section
            .find("irv_isvalidmetadataheader_stub")
            .expect("IsValidMetadataHeader stub missing from vtable");
        let auth_pos = vtable_section
            .find("irv_authenticate_stub")
            .expect("Authenticate stub missing from vtable");
        assert!(
            is_valid_pos < auth_pos,
            "IsValidMetadataHeader must appear before Authenticate in the vtable"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression: a virtual method returning a struct/class larger than
    /// 8 bytes must lower to an sret pointer parameter under the MSVC ABI.
    /// The stub signature has to match (`define void` + `ptr sret(...)`
    /// first parameter) or SAW rejects the module when it's combined
    /// with the real bitcode.
    #[test]
    fn test_vtable_stub_uses_sret_for_large_struct_return() {
        let large_struct = TypeInfo::Struct {
            name: "std::tuple<KeyStoreOperationResult,LatchableKey>".into(),
            size_bytes: Some(48),
            fields: vec![],
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", large_struct, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &std::collections::HashSet::new(),
            &dir,
            None,
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();

        // Stub must be void-returning with sret first param.
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        assert!(
            stub_line.contains("define void"),
            "sret return must be lowered to `void`, got: {stub_line}",
        );
        assert!(
            stub_line.contains("sret([48 x i8])"),
            "sret type must include struct byte size, got: {stub_line}",
        );
        assert!(
            stub_line.contains("%retptr"),
            "sret param must be named %retptr, got: {stub_line}",
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Negative regression: small scalar returns (e.g. `i32`) must NOT
    /// gain an sret parameter — that would break the calling convention
    /// for normal returns.
    #[test]
    fn test_vtable_stub_no_sret_for_scalar_return() {
        let methods = vec![make_iface_method(
            "IFoo",
            "Tick",
            TypeInfo::SignedInt(32),
            100,
        )];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_no_sret");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &std::collections::HashSet::new(),
            &dir,
            None,
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ifoo_tick_stub"))
            .expect("Tick stub missing");
        assert!(stub_line.contains("define i32"), "got: {stub_line}");
        assert!(!stub_line.contains("sret"), "got: {stub_line}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression: even when the AST has no `size_bytes` for the return
    /// type (e.g. `std::tuple<KeyStoreOperationResult, LatchableKey>`
    /// comes through as `Opaque{size_bytes: 0}` because the bitcode
    /// rewriter hasn't merged in a size yet), the stub must still lower
    /// the aggregate return via sret. Templated/qualified C++ names are
    /// always class types under MSVC, so falling back to a scalar `i32`
    /// return would mismatch the real ABI signature.
    #[test]
    fn test_vtable_stub_uses_sret_for_unsized_tuple_return() {
        let tuple_ret = TypeInfo::Opaque {
            name: "std::tuple<KeyStoreOperationResult,LatchableKey>".into(),
            size_bytes: 0,
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", tuple_ret, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret_unsized");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &std::collections::HashSet::new(),
            &dir,
            None,
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        assert!(
            stub_line.contains("define void"),
            "unsized aggregate return must still lower to void, got: {stub_line}",
        );
        assert!(
            stub_line.contains("sret(["),
            "must use a scratch-buffer sret type, got: {stub_line}",
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression: the MSVC x64 ABI orders parameters as
    /// `this, sret_ptr, args...` for instance methods. Putting the sret
    /// pointer at position 0 (before `this`) would shift the `this`
    /// argument and silently mis-dispatch when the stub is called via
    /// a vtable slot.
    #[test]
    fn test_vtable_stub_sret_ordered_after_this() {
        let large_struct = TypeInfo::Struct {
            name: "std::tuple<KeyStoreOperationResult,LatchableKey>".into(),
            size_bytes: Some(48),
            fields: vec![],
        };
        let methods = vec![make_iface_method("IKeyStore", "Read", large_struct, 100)];
        let dir = std::env::temp_dir().join("saw_spec_gen_vtable_sret_order");
        let _ = fs::remove_dir_all(&dir);
        emit_interface_stubs(
            &methods,
            &[],
            &[],
            &std::collections::HashSet::new(),
            &dir,
            None,
        )
        .unwrap();
        let ll = fs::read_to_string(dir.join("vtable_stubs.ll")).unwrap();
        let stub_line = ll
            .lines()
            .find(|l| l.contains("@ikeystore_read_stub"))
            .expect("Read stub missing");
        let this_pos = stub_line.find("%arg0").expect("this param missing");
        let sret_pos = stub_line.find("%retptr").expect("sret param missing");
        assert!(
            this_pos < sret_pos,
            "`this` (%arg0) must appear before sret (%retptr): {stub_line}",
        );
    }

    /// Regression: a havoc spec for an sret-returning method must
    /// allocate an output buffer, splice it into the argument list at
    /// position 1 (after `this`), and constrain `*result_ptr` via
    /// `llvm_points_to` rather than `llvm_return`. `llvm_return` on a
    /// void-returning stub is a type error in SAW.
    #[test]
    fn test_havoc_spec_uses_points_to_for_sret_return() {
        let tuple_ret = TypeInfo::Opaque {
            name: "std::tuple<KeyStoreOperationResult,LatchableKey>".into(),
            size_bytes: 0,
        };
        let method = make_iface_method("IKeyStore", "Read", tuple_ret, 100);
        let spec = generate_havoc_spec(&method, &[], None, None);
        assert!(
            spec.contains("result_ptr <- llvm_alloc"),
            "sret havoc spec must allocate result_ptr; spec was:\n{spec}",
        );
        assert!(
            spec.contains("llvm_points_to result_ptr"),
            "sret havoc spec must constrain *result_ptr via llvm_points_to; spec was:\n{spec}",
        );
        assert!(
            !spec.contains("llvm_return"),
            "sret havoc spec must not emit llvm_return; spec was:\n{spec}",
        );
        // result_ptr must follow this_ptr in the execute_func arg list.
        let exec_line = spec
            .lines()
            .find(|l| l.contains("llvm_execute_func"))
            .expect("execute_func missing");
        let this_pos = exec_line.find("this_ptr").expect("this_ptr missing in args");
        let result_pos = exec_line
            .find("result_ptr")
            .expect("result_ptr missing in args");
        assert!(
            this_pos < result_pos,
            "result_ptr must follow this_ptr in execute_func args: {exec_line}",
        );
    }
}
