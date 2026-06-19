use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::cli_gen_verify_args::{GenVerifyArgs, GenVerifyRustArgs, VerifyRustRunArgs};

/// Auto-generate SAW verification specs from C++ AST and Rust MIR type information.
///
/// Reads compiler-provided type info (clang -ast-dump=json, mir-json output, LLVM IR)
/// and generates SAW override specs with the tightest correct constraints derivable
/// from the type system, annotations, and compiler attributes.
#[derive(Parser)]
#[command(name = "saw-spec-gen", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
    GenVerify(#[command(flatten)] GenVerifyArgs),

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
    /// Usage: saw-spec-gen gen-verify-rust \
    ///          --llvm-ir add_one.ll --bitcode add_one.bc \
    ///          --cryptol-spec add_one_spec.cry --cryptol-fn add_one_spec \
    ///          --function add_one --output out_rust_add_one/
    GenVerifyRust(#[command(flatten)] GenVerifyRustArgs),

    /// Run the full native Rust verification pipeline:
    /// `rustc -> llvm-dis -> gen-verify-rust -> saw`.
    ///
    /// On DISPROVED, evaluates the Cryptol spec and re-runs the Rust
    /// implementation at the counterexample inputs, then writes
    /// `result.json` in schema version `1`.
    VerifyRust(#[command(flatten)] VerifyRustRunArgs),

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

    /// Aggregate `inventory.json` fragments into one
    /// `implementation_inventory.json` sidecar.
    AggregateInventory {
        /// Directory to scan recursively for `inventory.json` files.
        verify_out_dir: PathBuf,

        /// Output inventory path.
        #[arg(long, default_value = "implementation_inventory.json")]
        output: PathBuf,
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
