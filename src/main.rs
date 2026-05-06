mod clang_ast;
mod mir_json;
mod saw_emit;
mod constraints;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Auto-generate SAW verification specs from C++ AST and Rust MIR type information.
///
/// Reads compiler-provided type info (clang -ast-dump=json, mir-json output) and
/// generates SAW override specs with the tightest correct constraints derivable
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
    },

    /// Generate SAW specs from LLVM bitcode attributes (any language)
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
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::FromClangAst { input, output, filter } => {
            eprintln!("Reading clang AST from: {}", input.display());
            let ast = clang_ast::parse_ast(&input)?;
            let functions = clang_ast::extract_functions(&ast, filter.as_deref())?;
            eprintln!("Found {} functions", functions.len());

            let specs = constraints::derive_constraints(&functions)?;
            saw_emit::emit_saw_specs(&specs, &output)?;
            eprintln!("Generated {} specs in {}", specs.len(), output.display());
        }
        Commands::FromMirJson { input, output, filter } => {
            eprintln!("Reading MIR JSON from: {}", input.display());
            let mir = mir_json::parse_mir(&input)?;
            let functions = mir_json::extract_functions(&mir, filter.as_deref())?;
            eprintln!("Found {} functions", functions.len());

            let specs = constraints::derive_constraints(&functions)?;
            saw_emit::emit_saw_specs(&specs, &output)?;
            eprintln!("Generated {} specs in {}", specs.len(), output.display());
        }
        Commands::FromLlvmIr { input, output, filter } => {
            eprintln!("LLVM IR parsing not yet implemented");
            eprintln!("For now, use from-clang-ast or from-mir-json");
            std::process::exit(1);
        }
    }

    Ok(())
}
