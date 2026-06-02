// All module bodies live in the library crate (`src/lib.rs`) so that
// `cargo test --lib` has a target to build. This binary just parses
// CLI args and dispatches into `saw_spec_gen::commands::*`.
use saw_spec_gen::commands;

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

        /// Emit override scaffolding for external calls made by filtered functions.
        ///
        /// Walks the callgraph of each matched function, identifies calls to
        /// `declare`d (external) symbols, and emits `llvm_unsafe_assume_spec`
        /// scaffold stubs with the correct mangled names and parameter types.
        /// Use this to bootstrap compositional verification of Rust async
        /// functions that call into stdlib/external crates.
        #[arg(long)]
        emit_overrides: bool,

        /// Target function for callgraph analysis (used with --emit-overrides).
        /// Emits overrides only for external calls reachable from this function.
        #[arg(long)]
        target: Option<String>,
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

        /// Override the byte size of a specific C++ type name when the
        /// post-processing pass can't resolve `llvm_alias "NAME"` from
        /// the AST or LLVM IR.  Pass `--alias-size NAME=BYTES` once per
        /// override; emits `llvm_array BYTES (llvm_int 8)` for that
        /// name.  Use this for types whose only `dereferenceable(N)`
        /// attribute lives in a separate bitcode module (e.g.
        /// `std::tuple<…>` sret returns from interface methods
        /// implemented in a different .bc file).
        #[arg(long = "alias-size", value_name = "NAME=BYTES", num_args = 0..)]
        alias_size: Vec<String>,

        /// Override the bit width of a specific enum type name when the
        /// AST is missing the `EnumDecl` definition (e.g. only a forward
        /// declaration is reachable).  Pass `--alias-enum NAME=BITS`
        /// once per override; emits `llvm_int BITS` for that name.
        #[arg(long = "alias-enum", value_name = "NAME=BITS", num_args = 0..)]
        alias_enum: Vec<String>,

        /// Emit `llvm_combine_modules` in the generated `verify.saw`
        /// instead of pre-linking `main.bc` + `vtable_stubs.bc` with
        /// `llvm-link` at gen time.
        ///
        /// Default (off): pre-link with `llvm-link` and have SAW load a
        /// single `code.combined.bc`. Works with the stock GaloisInc
        /// SAW v1.5 release tarball.
        ///
        /// On (this flag): keep the old behavior. Produces a script
        /// that needs SAW master / a fork that includes the
        /// `llvm_combine_modules` primitive (merged upstream after the
        /// v1.5 tag).
        ///
        /// Has no effect when there are no interface (virtual) methods
        /// to stub — the single-module load path is always used in that
        /// case.
        #[arg(long = "use-llvm-combine-modules", default_value_t = false)]
        use_llvm_combine_modules: bool,
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

    /// Generate a SAW verification script + meta sidecar that proves
    /// a Rust function (compiled to LLVM bitcode) matches a Cryptol
    /// spec. The Rust analog of `gen-verify`.
    ///
    /// Walks the disassembled IR for `--llvm-ir`, resolves the
    /// mangled symbol whose name segment matches `--function`, builds
    /// a `(iN, …) -> iN` SAWScript with the same Bit/`[1]` Cryptol
    /// bridge the C++ generator uses, and writes:
    ///
    /// * `verify_rust.saw` — runnable SAW script
    /// * `verify_rust.meta.json` — mangled name, arg bit widths,
    ///   globals; consumed by `verify-rust.ps1` for counterexample
    ///   pretty-printing.
    ///
    /// Usage: saw-spec-gen gen-verify-rust \
    ///          --llvm-ir add_one.ll --bitcode add_one.bc \
    ///          --cryptol-spec add_one_spec.cry --cryptol-fn add_one_spec \
    ///          --function add_one --output out_rust_add_one/
    GenVerifyRust {
        /// Path to the disassembled LLVM IR (`.ll`) produced by
        /// `llvm-dis` from the same bitcode passed to `--bitcode`.
        #[arg(long = "llvm-ir")]
        llvm_ir: PathBuf,

        /// Path to the LLVM bitcode (`.bc`) the SAW script will
        /// `llvm_load_module`.
        #[arg(long)]
        bitcode: PathBuf,

        /// Cryptol spec file (`.cry`) copied next to the script.
        #[arg(long = "cryptol-spec")]
        cryptol_spec: PathBuf,

        /// Name of the Cryptol function to verify the Rust fn against.
        #[arg(long = "cryptol-fn")]
        cryptol_fn: String,

        /// Source-level Rust function name (e.g. `add_one`).
        #[arg(long)]
        function: String,

        /// Output directory for `verify_rust.saw`,
        /// `verify_rust.meta.json`, and the copied Cryptol spec.
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Strip system-header decls from a clang AST dump.
    ///
    /// Reads `--input` JSON, drops every top-level declaration whose
    /// source file lives outside any of the `--keep` directories, and
    /// writes the filtered AST to `--output`. The check is purely
    /// path-prefix based -- no allowlist of vendor headers required.
    ///
    /// Typical use is as a pre-pass before `gen-verify` when the raw
    /// AST is too large (e.g. when the translation unit `#include`s a
    /// templated STL header). For the demo:
    ///
    ///     saw-spec-gen filter-ast \
    ///         --input  big_ast.json \
    ///         --output small_ast.json \
    ///         --keep   tests/e2e/cases/05-string-ops/count_digits
    FilterAst {
        /// Path to the raw clang AST dump (any size).
        #[arg(long)]
        input: PathBuf,

        /// Path the filtered AST will be written to. May be the same
        /// as `--input`; the rewrite is atomic.
        #[arg(long)]
        output: PathBuf,

        /// Directory whose contents to keep. Pass `--keep` multiple
        /// times to union several roots (e.g. the .cpp's directory
        /// plus a third-party library you DO want introspected).
        #[arg(long, num_args = 1.., action = clap::ArgAction::Append, required = true)]
        keep: Vec<PathBuf>,
    },

    /// Patch an LLVM IR `.ll` file so SAW 1.5 / Crucible can load it.
    ///
    /// Two independent passes, each opt-in:
    ///
    /// * `--strip-msvc-eh` -- replace MSVC C++ exception-handling
    ///   metadata globals (`_TI*`, `_CTA*`, `_CT??_R0*` in
    ///   `section ".xdata"`) with `external constant` declarations.
    ///   Their initializers use `ptrtoint(@__ImageBase)` differences
    ///   which Crucible rejects at module-load time.
    ///
    /// * `--poison-to-undef` -- replace LLVM `poison` literals with
    ///   `undef`. Recent rustc/clang emit `insertvalue
    ///   { ..., T poison }, T %x, N` patterns; Crucible panics when
    ///   the partial aggregate is materialised.
    ///
    /// Pipeline: `clang -S -emit-llvm` -> `patch-llvm-ir` ->
    /// `llvm-as` -> SAW.
    ///
    /// Usage: saw-spec-gen patch-llvm-ir --input in.ll --output out.ll
    PatchLlvmIr {
        /// Input `.ll` file.
        #[arg(long)]
        input: PathBuf,

        /// Output `.ll` file. May be the same path as `--input`.
        #[arg(long)]
        output: PathBuf,

        /// Insert `store zeroinitializer` after every static `alloca`
        /// so that Crucible never sees an undef load from an
        /// uninitialized stack slot. **Opt-in** because it narrows
        /// the set of possible behaviours on those slots (undef → 0);
        /// do NOT enable when proving UB-freedom or absence-of-info-leak.
        #[arg(long, default_value_t = false)]
        init_undef_allocas: bool,
    },

    /// Aggregate per-run `result.json` files into a single
    /// `proof_manifest.json` for `pretty-specs --proof-status`.
    ///
    /// Walks `--root` recursively, reads every `result.json` (produced
    /// by `verify.ps1` / `verify-rust.ps1` / `verify-equiv.ps1`),
    /// validates the `schema_version`, and emits one manifest entry
    /// per file mapped to `proven` / `failed` / `not_attempted`.
    ///
    /// Usage: saw-spec-gen collect-results --root runs/ \
    ///          --output proof_manifest.json
    CollectResults {
        /// Directory to scan recursively for `result.json` files.
        #[arg(long)]
        root: PathBuf,

        /// Output manifest path.
        #[arg(long, default_value = "proof_manifest.json")]
        output: PathBuf,

        /// Optional JSON map `{ "impl_fn": "cryptol_fn", ... }` used to
        /// re-key entries when the implementation function name
        /// differs from the Cryptol `Item::Function.name`.
        #[arg(long)]
        cryptol_fn_map: Option<PathBuf>,

        /// Output shape: `flat` (default) emits a single array;
        /// `structured` groups entries by key (reserved for the
        /// pretty-specs-ua2 closed-loop integration).
        #[arg(long, default_value = "flat")]
        format: String,
    },

    /// Serialize per-function parameter / return type information and
    /// referenced struct layouts to a single `types.json` document.
    ///
    /// Reads any combination of `--ast`, `--mir`, and `--llvm-ir` (at
    /// least one required), runs the same extraction pipeline as
    /// `gen-verify`, and writes the schema-versioned JSON used by
    /// downstream tools (notably `pretty-specs`) for type-aware spec
    /// rendering.
    ///
    /// Usage: saw-spec-gen dump-types --ast ast.json \
    ///          --llvm-ir build/out.ll --output types.json
    DumpTypes {
        /// Clang AST JSON (`clang -Xclang -ast-dump=json`).
        #[arg(long)]
        ast: Option<PathBuf>,

        /// mir-json output (`linked-mir.json`).
        #[arg(long)]
        mir: Option<PathBuf>,

        /// LLVM textual IR (`.ll`).
        #[arg(long = "llvm-ir")]
        llvm_ir: Option<PathBuf>,

        /// Output `types.json` path.
        #[arg(long, default_value = "types.json")]
        output: PathBuf,

        /// Optional substring filter applied independently to each
        /// parser (same semantics as `gen-verify --filter`).
        #[arg(long)]
        filter: Option<String>,
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
        } => commands::gen_verify_cmd(
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
        } => commands::gen_verify_rust_cmd(
            llvm_ir,
            bitcode,
            cryptol_spec,
            cryptol_fn,
            function,
            output,
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
