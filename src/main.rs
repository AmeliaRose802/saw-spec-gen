mod clang_ast;
mod constraints;
mod cryptol_emit;
mod gen_verify;
mod llvm_ir;
mod mangle;
mod mir_json;
mod rust_trait_emit;
mod saw_emit;
mod spec_rewrite;
mod type_resolve;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Auto-generate SAW verification specs from C++ AST and Rust MIR type information.
///
/// Reads compiler-provided type info (clang -ast-dump=json, mir-json output, LLVM IR)
/// and generates SAW override specs with the tightest correct constraints derivable
/// from the type system, annotations, and compiler attributes.
#[derive(Parser)]
#[command(name = "saw-spec-gen", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate SAW specs from a clang AST dump (C/C++)
    ///
    /// Usage: saw-spec-gen from-clang-ast --input ast.json --output specs/
    FromClangAst {
        /// Path to clang -ast-dump=json output
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for generated .saw files
        #[arg(short, long)]
        output: PathBuf,

        /// Only generate specs for functions matching this pattern
        #[arg(short, long)]
        filter: Option<String>,

        /// Also generate Cryptol type constraint files
        #[arg(long)]
        cryptol: bool,

        /// Generate vtable stubs + havoc SAW specs for virtual methods
        ///
        /// Produces vtable_stubs.c (compile to .bc and combine with llvm_combine_modules)
        /// and havoc specs where mutable memory is explicitly havoced by the solver.
        #[arg(long)]
        emit_stubs: bool,

        /// Use SAW experimental builtins (llvm_unspecified_globals) for virtual/external specs.
        ///
        /// Requires SAW with enable_experimental. Generates simpler specs that use
        /// llvm_unspecified_globals instead of manual havoc specs for functions without
        /// a body or with virtual dispatch.
        #[arg(long)]
        experimental: bool,
    },

    /// Generate SAW specs from mir-json output (Rust)
    ///
    /// Usage: saw-spec-gen from-mir-json --input crate.mir.json --output specs/
    FromMirJson {
        /// Path to mir-json output file
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for generated .saw files
        #[arg(short, long)]
        output: PathBuf,

        /// Only generate specs for functions matching this pattern
        #[arg(short, long)]
        filter: Option<String>,

        /// Emit mir_verify specs instead of llvm_verify specs
        #[arg(long)]
        mir_verify: bool,

        /// Also generate Cryptol type constraint files
        #[arg(long)]
        cryptol: bool,
    },

    /// Generate SAW specs from LLVM IR text (any language)
    ///
    /// Usage: saw-spec-gen from-llvm-ir --input module.ll --output specs/
    FromLlvmIr {
        /// Path to LLVM IR (.ll) file (use llvm-dis to convert .bc to .ll)
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for generated .saw files
        #[arg(short, long)]
        output: PathBuf,

        /// Only generate specs for functions matching this pattern
        #[arg(short, long)]
        filter: Option<String>,

        /// Also generate Cryptol type constraint files
        #[arg(long)]
        cryptol: bool,
    },

    /// Generate a complete SAW verification script that checks a C++ function
    /// against a Cryptol spec.
    ///
    /// Runs the full pipeline: parses AST, generates override specs, vtable stubs,
    /// and emits a single runnable .saw file that loads bitcode, includes all
    /// overrides, imports the Cryptol spec, and calls llvm_verify.
    ///
    /// Usage: saw-spec-gen gen-verify --ast ast.json --bitcode code.bc \
    ///          --cryptol-spec spec.cry --cryptol-fn add_one_spec \
    ///          --function add_one --output verify.saw
    GenVerify {
        /// Path(s) to clang -ast-dump=json output.
        ///
        /// Pass `--ast` multiple times to merge interface headers with the
        /// translation unit holding the target function — gen-verify needs
        /// the interface ASTs to generate vtable stubs for virtual calls
        /// through `this->member` smart pointers.
        #[arg(long, num_args = 1.., action = clap::ArgAction::Append, required = true)]
        ast: Vec<PathBuf>,

        /// Path to LLVM bitcode (.bc) file
        #[arg(long)]
        bitcode: PathBuf,

        /// Optional path to LLVM IR text (.ll) of the same module as `--bitcode`.
        ///
        /// When provided, gen-verify scans it for `%struct.X = type { ... }`
        /// definitions and substitutes any opaque/unsized struct parameter type
        /// with a sized byte array (`llvm_array N (llvm_int 8)`).  Needed for
        /// MSVC-clang output where struct symbols are fully namespace-qualified
        /// (`%"struct.Foo::Bar::Baz"`) and the AST only knows the short name.
        #[arg(long)]
        llvm_ir: Option<PathBuf>,

        /// Path to Cryptol spec file (.cry)
        #[arg(long)]
        cryptol_spec: PathBuf,

        /// Name of the Cryptol function to check equivalence against
        #[arg(long)]
        cryptol_fn: String,

        /// C++ function name to verify (unmangled, e.g. "add_one")
        #[arg(long)]
        function: String,

        /// Output directory for all generated files (specs, stubs, verify script)
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Generate Rust trait vtable stubs + havoc specs for opaque
    /// `&dyn Trait` parameters (Rust analog of `from-clang-ast
    /// --emit-stubs`).
    ///
    /// Reads a typed schema (see src/rust_trait_emit.rs::TraitSchema)
    /// and emits `trait_stubs.ll` + `interface_overrides.saw` ready to
    /// `llvm-link` into the bitcode and `include` from the verify
    /// script.
    ///
    /// Usage: saw-spec-gen gen-rust-trait-stubs \
    ///          --schema my_traits.json --output out/
    GenRustTraitStubs {
        /// Path to a TraitSchema JSON file.
        #[arg(long)]
        schema: PathBuf,

        /// Output directory for generated files.
        #[arg(short, long)]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::FromClangAst {
            input,
            output,
            filter,
            cryptol,
            emit_stubs,
            experimental,
        } => {
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
                let vmethods = clang_ast::extract_virtual_methods(&ast, filter.as_deref())?;
                if vmethods.is_empty() {
                    eprintln!("No virtual methods found");

                    // Check if there are missing interface types that weren't in the AST
                    let missing = clang_ast::detect_missing_interfaces(&ast);
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
                } else {
                    // Collect class names that have virtual methods.
                    // `extract_virtual_methods` recognises both clang
                    // `virtual: true` and `OverrideAttr` markers, so this
                    // already covers derived classes overriding base methods.
                    let class_names: Vec<String> = vmethods
                        .iter()
                        .map(|m| m.class_name.clone())
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect();
                    let ctors = clang_ast::extract_constructors(&ast, &class_names)?;

                    // Collect all file-scope globals from the AST directly.
                    // Any virtual method could write to any global, so havoc
                    // specs need the full list — not just ones referenced by
                    // filtered functions.
                    let all_globals = clang_ast::extract_all_globals(&ast)?;

                    // Detect which classes have explicit virtual destructors
                    let classes_with_vdtor = clang_ast::classes_with_virtual_dtor(&ast);

                    saw_emit::emit_interface_stubs(&vmethods, &ctors, &all_globals, &classes_with_vdtor, &output, None)?;
                    eprintln!(
                        "Generated {} interface stubs in {}",
                        vmethods.len(),
                        output.display(),
                    );
                    if !ctors.is_empty() {
                        eprintln!(
                            "Generated {} constructor overrides",
                            ctors.len(),
                        );
                    }
                }
            }
        }
        Commands::FromMirJson {
            input,
            output,
            filter,
            mir_verify,
            cryptol,
        } => {
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
        }
        Commands::FromLlvmIr {
            input,
            output,
            filter,
            cryptol,
        } => {
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
        }
        Commands::GenVerify {
            ast,
            bitcode,
            llvm_ir,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
        } => {
            gen_verify::run(
                &ast,
                &bitcode,
                llvm_ir.as_deref(),
                &cryptol_spec,
                &cryptol_fn,
                &function,
                &output,
            )?;
        }
        Commands::GenRustTraitStubs { schema, output } => {
            eprintln!("Reading trait schema: {}", schema.display());
            rust_trait_emit::emit_trait_stubs(&schema, &output)?;
            eprintln!(
                "Wrote trait_stubs.ll and interface_overrides.saw to {}",
                output.display(),
            );
        }
    }

    Ok(())
}
