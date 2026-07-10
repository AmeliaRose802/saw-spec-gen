mod cli;
mod cli_gen_verify_args;

use cli::{Cli, Commands};
use saw_spec_gen::commands;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Verify(args) => commands::verify_cmd(
            args.cpp_file,
            args.cryptol_spec,
            args.cryptol_fn,
            args.function,
            args.output,
            args.include_dirs,
            args.cxx_standard,
            args.clang_flags,
            args.config,
            args.in_buffer_size,
            args.out_buffer_param,
            args.cryptol_fn_out,
            args.max_len_precond,
            args.no_struct_shape_recognizer,
            args.spec_only_on_missing,
        ),
        Commands::FromClangAst {
            input,
            output,
            filter,
            cryptol,
            emit_stubs,
            experimental,
        } => commands::from_clang_ast(input, output, filter, cryptol, emit_stubs, experimental),
        Commands::FromMirJson {
            input,
            output,
            filter,
            mir_verify,
            cryptol,
        } => commands::from_mir_json(input, output, filter, mir_verify, cryptol),
        Commands::FromLlvmIr {
            input,
            output,
            filter,
            cryptol,
            emit_overrides,
            target,
        } => commands::from_llvm_ir(input, output, filter, cryptol, emit_overrides, target),
        Commands::GenVerify(args) => commands::gen_verify_cmd(
            args.lang,
            args.ast,
            args.bitcode,
            args.llvm_ir,
            args.cryptol_spec,
            args.cryptol_fn,
            args.function,
            args.output,
            args.alias_size,
            args.alias_enum,
            args.use_llvm_combine_modules,
            args.spec_only_on_missing,
            args.in_buffer_size,
            args.out_buffer_param,
            args.cryptol_fn_out,
            args.cryptol_fn_pre,
            args.max_len_precond,
            args.cryptol_arg_order,
            args.variant_map,
            args.loop_invariants,
            args.no_struct_shape_recognizer,
            args.container_layouts,
            args.config,
        ),
        Commands::GenRustTraitStubs { schema, output } => {
            commands::gen_rust_trait_stubs(schema, output)
        }
        Commands::GenVerifyRust(args) => commands::gen_verify_rust_cmd(
            args.llvm_ir,
            args.bitcode,
            args.cryptol_spec,
            args.cryptol_fn,
            args.function,
            args.output,
            args.spec_only_on_missing,
            args.in_buffer_size,
            args.out_buffer_param,
            args.cryptol_fn_out,
            args.cryptol_fn_pre,
            args.max_len_precond,
            args.cryptol_arg_order,
            args.variant_map,
        ),
        Commands::VerifyRust(args) => {
            let code = commands::verify_rust_cmd(saw_spec_gen::verify_rust::VerifyRustArgs {
                rust_file: args.rust_file,
                cryptol_spec: args.cryptol_spec,
                cryptol_fn: args.cryptol_fn,
                function: args.function,
                output_dir: args.output,
                spec_only_on_missing: args.spec_only_on_missing,
            })?;
            std::process::exit(code);
        }
        Commands::FilterAst {
            input,
            output,
            keep,
        } => commands::filter_ast(input, output, keep),
        Commands::PatchLlvmIr {
            input,
            output,
            init_undef_allocas,
        } => commands::patch_llvm_ir_cmd(input, output, init_undef_allocas),
        Commands::CollectResults {
            root,
            output,
            cryptol_fn_map,
            format,
        } => {
            let fmt = saw_spec_gen::collect_results::ManifestFormat::parse(&format)?;
            saw_spec_gen::collect_results::run(&root, &output, cryptol_fn_map.as_deref(), fmt)
        }
        Commands::AggregateInventory {
            verify_out_dir,
            output,
        } => saw_spec_gen::inventory::aggregate_inventory(&verify_out_dir, &output),
        Commands::DumpTypes {
            ast,
            mir,
            llvm_ir,
            output,
            filter,
        } => saw_spec_gen::dump_types::run(
            ast.as_deref(),
            mir.as_deref(),
            llvm_ir.as_deref(),
            &output,
            filter.as_deref(),
        ),
    }
}
