//! Argument structs for the `verify`, `gen-verify`, and `gen-verify-rust`
//! subcommands.
//!
//! Extracted from `cli.rs` so that each source file stays under the
//! 500 non-whitespace line limit mandated by `.github/copilot-instructions.md`.

use clap::Args;
use std::path::PathBuf;

/// Arguments for the native `verify` subcommand (C++).
#[derive(Args)]
pub struct VerifyArgs {
    /// Path to the C++ source file.
    #[arg(long = "cpp-file")]
    pub cpp_file: PathBuf,

    /// Path to the Cryptol spec file (.cry).
    #[arg(long = "cryptol-spec")]
    pub cryptol_spec: PathBuf,

    /// Name of the Cryptol function to check against.
    #[arg(long = "cryptol-fn")]
    pub cryptol_fn: String,

    /// Name of the C++ function to verify (unmangled source name).
    #[arg(long)]
    pub function: String,

    /// Output directory for generated artifacts. Defaults to
    /// `out_<basename>` next to `--cpp-file`.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Extra `-I` include directories passed to every clang invocation.
    #[arg(long = "include-dir", num_args = 0.., action = clap::ArgAction::Append)]
    pub include_dirs: Vec<PathBuf>,

    /// Optional C++ standard passed as `-std=<value>`.
    #[arg(long = "cxx-standard")]
    pub cxx_standard: Option<String>,

    /// Additional raw flags appended verbatim to every clang invocation.
    #[arg(long = "clang-flag", num_args = 0.., action = clap::ArgAction::Append)]
    pub clang_flags: Vec<String>,

    /// Path to a per-invocation config file (TOML) forwarded to
    /// `gen-verify`.
    ///
    /// When omitted, `verify-cpp` preserves `gen-verify`'s normal
    /// spec-relative auto-discovery by passing the original
    /// `--cryptol-spec` path through unchanged. All spec shaping
    /// (buffer sizes, out-buffer bindings, length preconditions,
    /// struct-shape toggles, spec-only-on-missing) lives in the config
    /// file — there are no shaping CLI flags.
    #[arg(long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// Arguments for the `gen-verify` subcommand (C++ and unified C++/Rust path).
#[derive(Args)]
pub struct GenVerifyArgs {
    /// Target language: `cpp` or `rust`. Auto-detected from
    /// inputs when omitted: `rust` if `--llvm-ir` is provided
    /// without `--ast`, `cpp` otherwise.
    #[arg(long, value_name = "cpp|rust")]
    pub lang: Option<String>,

    /// Path(s) to clang -ast-dump=json output (C++ path).
    ///
    /// Pass `--ast` multiple times to merge interface headers with the
    /// translation unit holding the target function — gen-verify needs
    /// the interface ASTs to generate vtable stubs for virtual calls
    /// through `this->member` smart pointers.
    /// Not required when `--lang rust` is used.
    #[arg(long, num_args = 1.., action = clap::ArgAction::Append)]
    pub ast: Vec<PathBuf>,

    /// Path to LLVM bitcode (.bc) file
    #[arg(long)]
    pub bitcode: PathBuf,

    /// Optional path to LLVM IR text (.ll) of the same module as `--bitcode`.
    ///
    /// When provided, gen-verify scans it for `%struct.X = type { ... }`
    /// definitions and substitutes any opaque/unsized struct parameter type
    /// with a sized byte array (`llvm_array N (llvm_int 8)`).  Needed for
    /// MSVC-clang output where struct symbols are fully namespace-qualified
    /// (`%"struct.Foo::Bar::Baz"`) and the AST only knows the short name.
    #[arg(long)]
    pub llvm_ir: Option<PathBuf>,

    /// Path to Cryptol spec file (.cry)
    #[arg(long)]
    pub cryptol_spec: PathBuf,

    /// Name of the Cryptol function to check equivalence against
    #[arg(long)]
    pub cryptol_fn: String,

    /// C++ function name to verify (unmangled, e.g. "add_one")
    #[arg(long)]
    pub function: String,

    /// Output directory for all generated files (specs, stubs, verify script)
    #[arg(short, long)]
    pub output: PathBuf,

    /// Path to a per-invocation config file (TOML).
    ///
    /// When omitted, saw-spec-gen looks for config in this order:
    ///   1. `<cryptol-spec-stem>.toml` — sibling of the `--cryptol-spec` file,
    ///      same name, `.toml` extension (e.g. `count_bytes_spec.toml` next to
    ///      `count_bytes_spec.cry`).  Ideal for per-spec settings in a repo
    ///      with multiple compositional specs.
    ///   2. `saw-spec-gen.toml` walking up from the spec's directory.
    ///   3. `saw-spec-gen.toml` walking up from the current working directory.
    ///
    /// All spec shaping — buffer sizes, out-buffer bindings, length
    /// preconditions, alias sizes/enums, variant maps, argument order,
    /// struct-shape toggles, `llvm_combine_modules`, and
    /// `spec-only-on-missing` — is declared in the config file. Global
    /// keys apply everywhere; `[functions.<cryptol_fn>]` tables (keyed by
    /// `--cryptol-fn`) carry per-function overrides. There are no shaping
    /// CLI flags.
    #[arg(long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// Arguments for the `gen-verify-rust` subcommand (legacy Rust-only alias).
#[derive(Args)]
pub struct GenVerifyRustArgs {
    /// Path to the disassembled LLVM IR (`.ll`) produced by
    /// `llvm-dis` from the same bitcode passed to `--bitcode`.
    #[arg(long = "llvm-ir")]
    pub llvm_ir: PathBuf,

    /// Path to the LLVM bitcode (`.bc`) the SAW script will
    /// `llvm_load_module`.
    #[arg(long)]
    pub bitcode: PathBuf,

    /// Cryptol spec file (`.cry`) copied next to the script.
    #[arg(long = "cryptol-spec")]
    pub cryptol_spec: PathBuf,

    /// Name of the Cryptol function to verify the Rust fn against.
    #[arg(long = "cryptol-fn")]
    pub cryptol_fn: String,

    /// Source-level Rust function name (e.g. `add_one`).
    #[arg(long)]
    pub function: String,

    /// Output directory for `verify_rust.saw`,
    /// `verify_rust.meta.json`, and the copied Cryptol spec.
    #[arg(short, long)]
    pub output: PathBuf,

    /// Path to a per-invocation config file (TOML).
    ///
    /// When omitted, config is auto-discovered from the `--cryptol-spec`
    /// location (sibling `<stem>.toml`, then `saw-spec-gen.toml` walking
    /// up). All spec shaping — buffer sizes, out-buffer bindings, length
    /// preconditions, variant maps, argument order, and
    /// `spec-only-on-missing` — lives in the config file, either as
    /// global keys or `[functions.<cryptol_fn>]` tables. There are no
    /// shaping CLI flags.
    #[arg(long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// Arguments for the native `verify-rust` pipeline runner.
#[derive(Args)]
pub struct VerifyRustRunArgs {
    /// Path to the Rust source file.
    #[arg(long = "rust-file")]
    pub rust_file: PathBuf,

    /// Path to Cryptol spec file (`.cry`).
    #[arg(long = "cryptol-spec")]
    pub cryptol_spec: PathBuf,

    /// Name of the Cryptol function to verify against.
    #[arg(long = "cryptol-fn")]
    pub cryptol_fn: String,

    /// Source-level Rust function name (e.g. `add_one`).
    #[arg(long)]
    pub function: String,

    /// Optional output directory (defaults to `out_rust_<basename>`).
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Path to a per-invocation config file (TOML).
    ///
    /// When omitted, config is auto-discovered from the `--cryptol-spec`
    /// location. `spec-only-on-missing` and all spec shaping live in the
    /// config file — there are no shaping CLI flags.
    #[arg(long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,
}
