mod cli;

use cli::{Cli, Commands};
use saw_spec_gen::commands;

use anyhow::Result;
use clap::Parser;

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
        Commands::GenVerify {
            lang,
            ast,
            bitcode,
            llvm_ir,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
            alias_size,
            alias_enum,
            use_llvm_combine_modules,
            spec_only_on_missing,
            in_buffer_size,
            out_buffer_param,
            cryptol_fn_out,
            max_len_precond,
            cryptol_arg_order,
            variant_map,
        } => commands::gen_verify_cmd(
            lang,
            ast,
            bitcode,
            llvm_ir,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
            alias_size,
            alias_enum,
            use_llvm_combine_modules,
            spec_only_on_missing,
            in_buffer_size,
            out_buffer_param,
            cryptol_fn_out,
            max_len_precond,
            cryptol_arg_order,
            variant_map,
        ),
        Commands::GenRustTraitStubs { schema, output } => {
            commands::gen_rust_trait_stubs(schema, output)
        }
        Commands::GenVerifyRust {
            llvm_ir,
            bitcode,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
            spec_only_on_missing,
            in_buffer_size,
            out_buffer_param,
            cryptol_fn_out,
            max_len_precond,
            cryptol_arg_order,
            variant_map,
        } => commands::gen_verify_rust_cmd(
            llvm_ir,
            bitcode,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
            spec_only_on_missing,
            in_buffer_size,
            out_buffer_param,
            cryptol_fn_out,
            max_len_precond,
            cryptol_arg_order,
            variant_map,
        ),
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
