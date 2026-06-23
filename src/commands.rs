//! Subcommand handler implementations for the `saw-spec-gen` binary.
//!
//! `main.rs` parses CLI args into [`Commands`] (defined alongside `Cli` in
//! `main.rs`) and immediately delegates here, so the binary's entry point
//! stays small and the command bodies remain editable without pushing
//! `main.rs` over the 500-non-whitespace-line limit.

use anyhow::Result;
use std::path::PathBuf;

use crate::{
    clang_ast, constraints, cryptol_emit, gen_verify, gen_verify_rust, llvm_ir, mir_json,
    patch_llvm_ir, rust_trait_emit, saw_emit, verify_cpp, verify_rust,
};

#[allow(clippy::too_many_arguments)]
pub fn verify_cmd(
    cpp_file: PathBuf,
    cryptol_spec: PathBuf,
    cryptol_fn: String,
    function: String,
    output: Option<PathBuf>,
    include_dirs: Vec<PathBuf>,
    cxx_standard: Option<String>,
    clang_flags: Vec<String>,
    extra_spec_gen_args: Vec<String>,
    spec_only_on_missing: bool,
) -> Result<()> {
    let outcome = verify_cpp::run(verify_cpp::VerifyRequest {
        cpp_file,
        cryptol_spec,
        cryptol_fn,
        function,
        output,
        include_dirs,
        cxx_standard,
        clang_flags,
        extra_spec_gen_args,
        spec_only_on_missing,
    })?;
    std::process::exit(outcome.exit_code);
}

pub fn from_clang_ast(
    input: PathBuf,
    output: PathBuf,
    filter: Option<String>,
    cryptol: bool,
    emit_stubs: bool,
    experimental: bool,
) -> Result<()> {
    eprintln!("Reading clang AST from: {}", input.display());
    let ast = clang_ast::parse_ast(&input)?;
    let functions = clang_ast::extract_functions(&ast, filter.as_deref())?;
    eprintln!("Found {} functions", functions.len());

    let specs = constraints::derive_constraints(&functions)?;

    // When experimental, collect ALL globals from the translation unit
    // so virtual/external specs can declare they may modify any of them.
    let all_globals = if experimental {
        clang_ast::extract_all_globals(&ast)?
    } else {
        vec![]
    };
    saw_emit::emit_saw_specs_with_globals(&specs, &output, experimental, &all_globals)?;
    eprintln!("Generated {} specs in {}", specs.len(), output.display());

    if cryptol {
        cryptol_emit::emit_cryptol_constraints(&functions, &output)?;
        eprintln!("Generated Cryptol constraints in {}", output.display());
    }

    if emit_stubs {
        emit_clang_ast_stubs(&ast, filter.as_deref(), &output)?;
    }
    Ok(())
}

fn emit_clang_ast_stubs(
    ast: &clang_ast::AstNode,
    filter: Option<&str>,
    output: &std::path::Path,
) -> Result<()> {
    let vmethods = clang_ast::extract_virtual_methods(ast, filter)?;
    if vmethods.is_empty() {
        eprintln!("No virtual methods found");

        // Check if there are missing interface types that weren't in the AST
        let missing = clang_ast::detect_missing_interfaces(ast);
        if !missing.is_empty() {
            eprintln!();
            eprintln!(
                "WARNING: Found {} interface type(s) referenced by class fields but missing from this AST:",
                missing.len(),
            );
            for m in &missing {
                eprintln!(
                    "  - {} (via field {}.{}, type {})",
                    m.interface_name, m.owning_class, m.field_name, m.wrapper,
                );
            }
            eprintln!();
            eprintln!("To generate vtable stubs for these, run additional passes:");
            for m in &missing {
                eprintln!(
                    "  clang -Xclang -ast-dump=json -Xclang -ast-dump-filter={} ... > {}_ast.json",
                    m.interface_name, m.interface_name,
                );
            }
            eprintln!();
        }
        return Ok(());
    }

    // Collect class names that have virtual methods. `extract_virtual_methods`
    // recognises both clang `virtual: true` and `OverrideAttr` markers, so this
    // already covers derived classes overriding base methods.
    let class_names: Vec<String> = vmethods
        .iter()
        .map(|m| m.class_name.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let ctors = clang_ast::extract_constructors(ast, &class_names)?;

    // Collect all file-scope globals from the AST directly. Any virtual method
    // could write to any global, so havoc specs need the full list — not just
    // ones referenced by filtered functions.
    let all_globals = clang_ast::extract_all_globals(ast)?;

    // Detect which classes have explicit virtual destructors.
    let classes_with_vdtor = clang_ast::classes_with_virtual_dtor(ast);

    saw_emit::emit_interface_stubs(
        &vmethods,
        &ctors,
        &all_globals,
        &classes_with_vdtor,
        output,
        None,
        None,
    )?;
    eprintln!(
        "Generated {} interface stubs in {}",
        vmethods.len(),
        output.display(),
    );
    if !ctors.is_empty() {
        eprintln!("Generated {} constructor overrides", ctors.len());
    }
    Ok(())
}

pub fn from_mir_json(
    input: PathBuf,
    output: PathBuf,
    filter: Option<String>,
    mir_verify: bool,
    cryptol: bool,
) -> Result<()> {
    eprintln!("Reading MIR JSON from: {}", input.display());
    let mir = mir_json::parse_mir(&input)?;
    let functions = mir_json::extract_functions(&mir, filter.as_deref())?;
    eprintln!("Found {} functions", functions.len());

    let specs = constraints::derive_constraints(&functions)?;
    if mir_verify {
        saw_emit::emit_mir_saw_specs(&specs, &output)?;
        eprintln!(
            "Generated {} MIR specs in {}",
            specs.len(),
            output.display()
        );
    } else {
        saw_emit::emit_saw_specs(&specs, &output, false)?;
        eprintln!("Generated {} specs in {}", specs.len(), output.display());
    }

    if cryptol {
        cryptol_emit::emit_cryptol_constraints(&functions, &output)?;
        eprintln!("Generated Cryptol constraints in {}", output.display());
    }
    Ok(())
}

pub fn from_llvm_ir(
    input: PathBuf,
    output: PathBuf,
    filter: Option<String>,
    cryptol: bool,
    emit_overrides: bool,
    target: Option<String>,
) -> Result<()> {
    eprintln!("Reading LLVM IR from: {}", input.display());
    let ir = llvm_ir::parse_llvm_ir(&input)?;
    let functions = llvm_ir::extract_functions(&ir, filter.as_deref())?;
    eprintln!("Found {} functions", functions.len());

    let specs = constraints::derive_constraints(&functions)?;
    saw_emit::emit_saw_specs(&specs, &output, false)?;
    eprintln!("Generated {} specs in {}", specs.len(), output.display());

    if cryptol {
        cryptol_emit::emit_cryptol_constraints(&functions, &output)?;
        eprintln!("Generated Cryptol constraints in {}", output.display());
    }

    if emit_overrides {
        emit_llvm_ir_overrides(&ir, filter.as_deref(), target.as_deref(), &output)?;
    }
    Ok(())
}

fn emit_llvm_ir_overrides(
    ir: &str,
    filter: Option<&str>,
    target: Option<&str>,
    output: &std::path::Path,
) -> Result<()> {
    use crate::parsers::llvm_ir::callgraph;
    let cg = callgraph::build_callgraph(ir);
    let target_name = target.or(filter).unwrap_or("");
    let external = callgraph::external_callees(&cg, target_name);

    if external.is_empty() {
        eprintln!("No external calls found for target '{}'", target_name);
        if !target_name.is_empty() {
            // Show available functions with callees
            let with_calls: Vec<_> = cg
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| {
                    format!(
                        "  {} ({} calls)",
                        k.chars().take(80).collect::<String>(),
                        v.len()
                    )
                })
                .collect();
            if !with_calls.is_empty() {
                eprintln!("Functions with calls (showing first 10):");
                for f in with_calls.iter().take(10) {
                    eprintln!("{}", f);
                }
            }
        }
        return Ok(());
    }

    // Generate scaffold override specs for external calls
    let overrides_dir = output.join("overrides");
    std::fs::create_dir_all(&overrides_dir)?;

    let mut all_overrides = String::new();
    all_overrides.push_str("// Auto-generated override scaffolding for external calls\n");
    all_overrides.push_str(&format!("// Target: {}\n", target_name));
    all_overrides.push_str(&format!(
        "// {} external calls identified\n\n",
        external.len()
    ));

    for (i, callee) in external.iter().enumerate() {
        let spec_name = format!("override_{}", i);
        let spec = format!(
            "// Override for: {name}\n\
             // Mangled: {mangled}\n\
             // TODO: Tighten this contract — currently returns any value.\n\
             //       For Option<T>, constrain discriminant to 0 or 1.\n\
             //       For Result<T,E>, constrain discriminant to valid range.\n\
             let {spec_name}_spec : LLVMSetup () = do {{\n\
             \n\
             // TODO: Add parameter allocations matching the LLVM signature.\n\
             //       Check the function signature in the .ll file:\n\
             //         grep 'declare.*{short}' module.ll\n\
             \n\
             llvm_execute_func [];\n\
             \n\
             // TODO: Specify return value.\n\
             //   ret <- llvm_fresh_var \"ret\" (llvm_int 32);\n\
             //   llvm_return (llvm_term ret);\n\
             }};\n\n\
             // ov_{spec_name} <- llvm_unsafe_assume_spec m \"{mangled}\" {spec_name}_spec;\n\n",
            name = callee.name,
            mangled = callee.mangled_name,
            spec_name = spec_name,
            short = callee.mangled_name.chars().take(40).collect::<String>(),
        );
        all_overrides.push_str(&spec);
    }

    let overrides_path = overrides_dir.join("external_overrides.saw");
    std::fs::write(&overrides_path, &all_overrides)?;
    eprintln!(
        "Generated {} external override scaffolds in {}",
        external.len(),
        overrides_path.display()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn gen_verify_cmd(
    lang: Option<String>,
    ast: Vec<PathBuf>,
    bitcode: PathBuf,
    llvm_ir: Option<PathBuf>,
    cryptol_spec: PathBuf,
    cryptol_fn: String,
    function: String,
    output: PathBuf,
    alias_size: Vec<String>,
    alias_enum: Vec<String>,
    use_llvm_combine_modules: bool,
    spec_only_on_missing: bool,
    in_buffer_size: Vec<String>,
    out_buffer_param: Vec<String>,
    cryptol_fn_out: Vec<String>,
    cryptol_fn_pre: Vec<String>,
    max_len_precond: Vec<String>,
    cryptol_arg_order: Vec<String>,
    variant_map: Vec<String>,
    bind_cryptol_lengths: bool,
    no_struct_shape_recognizer: bool,
    container_layouts: Option<PathBuf>,
) -> Result<()> {
    // Auto-detect language: Rust when --llvm-ir is provided without --ast
    let effective_lang = match lang.as_deref() {
        Some("rust") => "rust",
        Some("cpp") => "cpp",
        Some(other) => anyhow::bail!("--lang must be 'cpp' or 'rust', got '{other}'"),
        None => {
            if ast.is_empty() && llvm_ir.is_some() {
                "rust"
            } else {
                "cpp"
            }
        }
    };

    if effective_lang == "rust" {
        let ir_path = llvm_ir.ok_or_else(|| anyhow::anyhow!("--lang rust requires --llvm-ir"))?;
        let overrides = crate::buffer_overrides::BufferOverrides::from_cli(
            &in_buffer_size,
            &out_buffer_param,
            &cryptol_fn_out,
            &max_len_precond,
            &cryptol_arg_order,
            &cryptol_fn_pre,
        )?;
        let vmap = crate::gen_verify_rust_emit::VariantMap::parse_all(&variant_map)?;
        return gen_verify_rust::run(
            &ir_path,
            &bitcode,
            &cryptol_spec,
            &cryptol_fn,
            &function,
            &output,
            spec_only_on_missing,
            &overrides,
            &vmap,
        );
    }

    // C++ path — --ast is required
    if ast.is_empty() {
        anyhow::bail!("--ast is required for C++ verification (use --lang rust for Rust)");
    }
    let buffer_overrides = crate::buffer_overrides::BufferOverrides::from_cli(
        &in_buffer_size,
        &out_buffer_param,
        &cryptol_fn_out,
        &max_len_precond,
        &cryptol_arg_order,
        &cryptol_fn_pre,
    )?;
    gen_verify::run(
        &ast,
        &bitcode,
        llvm_ir.as_deref(),
        &cryptol_spec,
        &cryptol_fn,
        &function,
        &output,
        &alias_size,
        &alias_enum,
        use_llvm_combine_modules,
        spec_only_on_missing,
        &buffer_overrides,
        bind_cryptol_lengths,
        no_struct_shape_recognizer,
        container_layouts.as_deref(),
    )
}

pub fn gen_rust_trait_stubs(schema: PathBuf, output: PathBuf) -> Result<()> {
    eprintln!("Reading trait schema: {}", schema.display());
    rust_trait_emit::emit_trait_stubs(&schema, &output)?;
    eprintln!(
        "Wrote trait_stubs.ll and interface_overrides.saw to {}",
        output.display(),
    );
    Ok(())
}

/// Implementation of `gen-verify-rust`. See [`gen_verify_rust::run`]
/// for the heavy lifting.
#[allow(clippy::too_many_arguments)]
pub fn gen_verify_rust_cmd(
    llvm_ir: PathBuf,
    bitcode: PathBuf,
    cryptol_spec: PathBuf,
    cryptol_fn: String,
    function: String,
    output: PathBuf,
    spec_only_on_missing: bool,
    in_buffer_size: Vec<String>,
    out_buffer_param: Vec<String>,
    cryptol_fn_out: Vec<String>,
    cryptol_fn_pre: Vec<String>,
    max_len_precond: Vec<String>,
    cryptol_arg_order: Vec<String>,
    variant_map: Vec<String>,
) -> Result<()> {
    let overrides = crate::buffer_overrides::BufferOverrides::from_cli(
        &in_buffer_size,
        &out_buffer_param,
        &cryptol_fn_out,
        &max_len_precond,
        &cryptol_arg_order,
        &cryptol_fn_pre,
    )?;
    let vmap = crate::gen_verify_rust_emit::VariantMap::parse_all(&variant_map)?;
    gen_verify_rust::run(
        &llvm_ir,
        &bitcode,
        &cryptol_spec,
        &cryptol_fn,
        &function,
        &output,
        spec_only_on_missing,
        &overrides,
        &vmap,
    )
}

pub fn verify_rust_cmd(args: verify_rust::VerifyRustArgs) -> Result<i32> {
    let outcome = verify_rust::run(args)?;
    Ok(outcome.exit_code())
}

pub fn filter_ast(input: PathBuf, output: PathBuf, keep: Vec<PathBuf>) -> Result<()> {
    let size_mb = std::fs::metadata(&input)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    eprintln!(
        "Filtering AST: {} ({} MB) -> {}",
        input.display(),
        size_mb,
        output.display(),
    );
    eprintln!("Keep prefixes:");
    for p in &keep {
        eprintln!("  {}", p.display());
    }
    let stats = clang_ast::filter_ast_file(&input, &output, &keep)?;
    eprintln!(
        "Filter result: kept {}, dropped {}, no-loc {}",
        stats.kept, stats.dropped, stats.no_loc,
    );
    Ok(())
}

pub fn patch_llvm_ir_cmd(input: PathBuf, output: PathBuf, init_undef_allocas: bool) -> Result<()> {
    eprintln!(
        "Patching LLVM IR: {} -> {}",
        input.display(),
        output.display(),
    );
    let stats = patch_llvm_ir::patch_llvm_ir_file(&input, &output, init_undef_allocas)?;
    eprintln!(
        "  EH globals stripped: {}, Itanium typeinfo stripped: {}, \
         poison→undef: {}, nsw/nuw stripped: {}, \
         sat intrinsics expanded: {}, allocas zeroed: {}",
        stats.eh_globals_stripped,
        stats.itanium_typeinfo_stripped,
        stats.poison_replaced,
        stats.nsw_nuw_stripped,
        stats.sat_intrinsics_expanded,
        stats.allocas_zeroed,
    );
    if stats.allocas_zeroed > 0 {
        eprintln!(
            "  ⚠ init-undef-allocas narrowed symbolic behavior on {} stack slot(s).\n    \
             Do NOT enable when proving UB-freedom or absence-of-info-leak.",
            stats.allocas_zeroed,
        );
    }
    Ok(())
}
