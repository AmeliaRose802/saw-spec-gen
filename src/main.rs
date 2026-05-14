mod clang_ast;
mod constraints;
mod cryptol_emit;
mod llvm_ir;
mod mir_json;
mod saw_emit;

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
        } => {
            eprintln!("Reading clang AST from: {}", input.display());
            let ast = clang_ast::parse_ast(&input)?;
            let functions = clang_ast::extract_functions(&ast, filter.as_deref())?;
            eprintln!("Found {} functions", functions.len());

            let specs = constraints::derive_constraints(&functions)?;
            saw_emit::emit_saw_specs(&specs, &output)?;
            eprintln!("Generated {} specs in {}", specs.len(), output.display());

            if cryptol {
                cryptol_emit::emit_cryptol_constraints(&functions, &output)?;
                eprintln!("Generated Cryptol constraints in {}", output.display());
            }

            if emit_stubs {
                let vmethods = clang_ast::extract_virtual_methods(&ast, filter.as_deref())?;
                if vmethods.is_empty() {
                    eprintln!("No virtual methods found");
                } else {
                    saw_emit::emit_interface_stubs(&vmethods, &output)?;
                    eprintln!(
                        "Generated {} interface stubs in {}",
                        vmethods.len(),
                        output.display(),
                    );
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
                saw_emit::emit_saw_specs(&specs, &output)?;
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
            saw_emit::emit_saw_specs(&specs, &output)?;
            eprintln!("Generated {} specs in {}", specs.len(), output.display());

            if cryptol {
                cryptol_emit::emit_cryptol_constraints(&functions, &output)?;
                eprintln!("Generated Cryptol constraints in {}", output.display());
            }
        }
    }

    Ok(())
}
