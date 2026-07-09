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
    /// `--cryptol-spec` path through unchanged. Prefer versioned
    /// config files for shaping.
    #[arg(long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Declare a read-only input buffer override.
    #[arg(long = "in-buffer-size", value_name = "NAME=SHAPE", num_args = 0..)]
    pub in_buffer_size: Vec<String>,

    /// Declare a writable output buffer override.
    #[arg(long = "out-buffer-param", value_name = "NAME=SHAPE|auto", num_args = 0..)]
    pub out_buffer_param: Vec<String>,

    /// Bind an out-buffer to a Cryptol postcondition function.
    #[arg(long = "cryptol-fn-out", value_name = "OUT_PARAM=FN", num_args = 0..)]
    pub cryptol_fn_out: Vec<String>,

    /// Emit a scalar length precondition before the call.
    #[arg(long = "max-len-precond", value_name = "NAME=VAL", num_args = 0..)]
    pub max_len_precond: Vec<String>,

    /// Disable the struct-shape recognizer.
    #[arg(long = "no-struct-shape-recognizer", default_value_t = false)]
    pub no_struct_shape_recognizer: bool,

    /// Soft-exit with a `result.json` status of `not_attempted` when the
    /// target function has no matching implementation symbol.
    #[arg(long = "spec-only-on-missing", default_value_t = false)]
    pub spec_only_on_missing: bool,
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

    /// Override the byte size of a specific C++ type name when the
    /// post-processing pass can't resolve `llvm_alias "NAME"` from
    /// the AST or LLVM IR.  Pass `--alias-size NAME=BYTES` once per
    /// override; emits `llvm_array BYTES (llvm_int 8)` for that
    /// name.  Use this for types whose only `dereferenceable(N)`
    /// attribute lives in a separate bitcode module (e.g.
    /// `std::tuple<…>` sret returns from interface methods
    /// implemented in a different .bc file).
    #[arg(long = "alias-size", value_name = "NAME=BYTES", num_args = 0..)]
    pub alias_size: Vec<String>,

    /// Override the bit width of a specific enum type name when the
    /// AST is missing the `EnumDecl` definition (e.g. only a forward
    /// declaration is reachable).  Pass `--alias-enum NAME=BITS`
    /// once per override; emits `llvm_int BITS` for that name.
    #[arg(long = "alias-enum", value_name = "NAME=BITS", num_args = 0..)]
    pub alias_enum: Vec<String>,

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
    pub use_llvm_combine_modules: bool,

    /// Soft-exit (writing a `result.json` with `status:
    /// not_attempted` and a human-readable `reason`) instead of
    /// erroring out when the Cryptol `--function` has no matching
    /// C++ symbol in the AST / no mangled name / no derivable
    /// constraint spec.
    ///
    /// Intended for batch pipelines that drive `gen-verify` over
    /// every top-level Cryptol definition in a spec module.
    #[arg(long = "spec-only-on-missing", default_value_t = false)]
    pub spec_only_on_missing: bool,

    /// Declare a C++ pointer parameter as a read-only input
    /// buffer of a known shape, instead of inferring a 1-byte
    /// alloc from the bare pointer type.
    ///
    /// Format: `NAME=SHAPE`, SHAPE = `BYTES` (byte buffer) |
    /// `iW` (single wide scalar field) | `NxiW` (wide array) |
    /// `struct:Type` (named LLVM struct) | `{f1,f2,...}`
    /// (heterogeneous struct, fields are themselves SHAPEs) |
    /// `<{f1,f2,...}>` (packed struct, no padding).
    #[arg(long = "in-buffer-size", value_name = "NAME=SHAPE", num_args = 0..)]
    pub in_buffer_size: Vec<String>,

    /// Declare a C++ pointer parameter as a writable output
    /// buffer of a known shape. The generated spec allocates a
    /// typed region, binds a fresh `<NAME>_pre` to its pre-call
    /// contents, and (with `--cryptol-fn-out NAME=FN`) post-asserts
    /// `llvm_points_to <NAME>_ptr (llvm_term {{ FN ... }})`.
    ///
    /// Format: `NAME=SHAPE` or `NAME=auto`. SHAPE = `BYTES`
    /// (byte buffer, for byte-granular fields) | `iW` (a single
    /// wide scalar field, e.g. a `uint32` loaded as one i32 a byte
    /// array can't model) | `NxiW` (wide array, e.g. `4xi32`) |
    /// `struct:Type` (named LLVM struct, e.g.
    /// `struct:EnrollmentKey`) | `{f1,f2,...}` (heterogeneous struct
    /// for mixed-width layouts, e.g. `{16xi8,i8}` for a Uuid+bool) |
    /// `<{f1,f2,...}>` (packed struct). `auto` keeps the inferred
    /// pointee type.
    #[arg(long = "out-buffer-param", value_name = "NAME=SHAPE|auto", num_args = 0..)]
    pub out_buffer_param: Vec<String>,

    /// Bind a Cryptol function to the post-call contents of an
    /// out-buffer declared with `--out-buffer-param`. The
    /// generated spec emits a `llvm_points_to` post-condition
    /// asserting the buffer holds `FN <args>` after the call.
    /// Argument ordering follows `--cryptol-arg-order FN=...`
    /// when supplied, else the positional default.
    ///
    /// Format: `OUT_PARAM=FN`. Pass once per out-buffer.
    #[arg(long = "cryptol-fn-out", value_name = "OUT_PARAM=FN", num_args = 0..)]
    pub cryptol_fn_out: Vec<String>,

    /// Optional Cryptol pre-state predicate emitted as
    /// `llvm_precond {{ FN ... }}` immediately before
    /// `llvm_execute_func`.
    ///
    /// Format: `FN`. Pass at most once.
    #[arg(long = "cryptol-fn-pre", value_name = "FN", num_args = 0..=1)]
    pub cryptol_fn_pre: Vec<String>,

    /// Emit an `llvm_precond {{ NAME <= VAL }}` constraint just
    /// before `llvm_execute_func`. Use to bound scalar length
    /// parameters to the declared buffer size.
    ///
    /// Format: `NAME=VAL`. Pass once per bound.
    #[arg(long = "max-len-precond", value_name = "NAME=VAL", num_args = 0..)]
    pub max_len_precond: Vec<String>,

    /// Override the Cryptol-side argument order for a function
    /// referenced via `--cryptol-fn` or `--cryptol-fn-out`.
    /// Each token is either `<param_name>` or `@pre.<param_name>`.
    ///
    /// Format: `FN=arg1,arg2,...`. Pass once per Cryptol fn.
    #[arg(long = "cryptol-arg-order", value_name = "FN=arg1,arg2,...", num_args = 0..)]
    pub cryptol_arg_order: Vec<String>,

    /// Restrict verification to a subset of enum variants when
    /// the impl has fewer variants than the canonical spec
    /// (Rust path only).
    /// Format: `PARAM=V1:disc1,V2:disc2,...` (e.g.
    /// `x0=Success:0,AlreadyActive:1`). Use `return=V1:D1,...`
    /// for a narrowing adapter on the return type.
    #[arg(long = "variant-map", value_name = "PARAM=V1:D1,V2:D2,...", num_args = 0..)]
    pub variant_map: Vec<String>,

    /// Disable the struct-shape recognizer (ArrayView rule 4).
    /// The recognizer pairs adjacent `(T* buf, size_t len)`
    /// parameters and synthesizes `_In_reads_(len)` on the buffer
    /// when neither carries a size annotation.
    #[arg(long = "no-struct-shape-recognizer", default_value_t = false)]
    pub no_struct_shape_recognizer: bool,

    /// Optional container-layout TOML catalog (ArrayView rule 5).
    /// Merged over the built-in defaults.
    /// **No-op today + scheduled for deletion** in favor of AST-
    /// driven auto-derivation (saw_spec_gen-530, -qms, -0nf).
    /// Passing this flag prints a stderr warning.
    #[arg(long = "container-layouts", value_name = "PATH")]
    pub container_layouts: Option<PathBuf>,

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
    /// Config values act as defaults; explicit CLI flags always override them.
    ///
    /// Per-function shaping can be declared in `[functions.<cryptol_fn>]`
    /// tables (keyed by `--cryptol-fn`), carrying the same buffer/shape
    /// flags. Resolution order is per-function config → global config →
    /// CLI, so a typed config replaces hand-coded driver-script flags.
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

    /// Soft-exit with `result.json` status `not_attempted`
    /// instead of erroring when `--function` has no matching
    /// Rust symbol in the LLVM IR.
    #[arg(long = "spec-only-on-missing", default_value_t = false)]
    pub spec_only_on_missing: bool,

    /// Read-only input buffer override. Format: `NAME=SHAPE`
    /// (SHAPE = `BYTES` | `iW` | `NxiW` | `{f1,f2,...}` |
    /// `<{f1,f2,...}>`).
    #[arg(long = "in-buffer-size", value_name = "NAME=SHAPE", num_args = 0..)]
    pub in_buffer_size: Vec<String>,

    /// Writable output buffer override. Format: `NAME=SHAPE`
    /// (SHAPE = `BYTES` | `iW` | `NxiW` | `{f1,f2,...}` |
    /// `<{f1,f2,...}>`) or `NAME=auto`.
    #[arg(long = "out-buffer-param", value_name = "NAME=SHAPE|auto", num_args = 0..)]
    pub out_buffer_param: Vec<String>,

    /// Cryptol fn for out-buffer postcondition. Format: `OUT_PARAM=FN`.
    #[arg(long = "cryptol-fn-out", value_name = "OUT_PARAM=FN", num_args = 0..)]
    pub cryptol_fn_out: Vec<String>,

    /// Optional Cryptol pre-state predicate emitted as
    /// `llvm_precond {{ FN ... }}` immediately before
    /// `llvm_execute_func`.
    #[arg(long = "cryptol-fn-pre", value_name = "FN", num_args = 0..=1)]
    pub cryptol_fn_pre: Vec<String>,

    /// Emit `llvm_precond {{ NAME <= VAL }}`. Format: `NAME=VAL`.
    #[arg(long = "max-len-precond", value_name = "NAME=VAL", num_args = 0..)]
    pub max_len_precond: Vec<String>,

    /// Explicit Cryptol argument order. Format: `FN=arg1,arg2,...`.
    #[arg(long = "cryptol-arg-order", value_name = "FN=arg1,arg2,...", num_args = 0..)]
    pub cryptol_arg_order: Vec<String>,

    /// Restrict verification to a subset of enum variants when
    /// the impl has fewer variants than the canonical spec.
    /// Format: `PARAM=V1:disc1,V2:disc2,...` (e.g.
    /// `x0=Success:0,AlreadyActive:1`). The generated spec
    /// emits a membership precondition restricting the parameter
    /// to the listed discriminants, and a narrowing adapter for
    /// the return value. Pass once per parameter.
    #[arg(long = "variant-map", value_name = "PARAM=V1:D1,V2:D2,...", num_args = 0..)]
    pub variant_map: Vec<String>,
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

    /// Soft-exit with `result.json` status `not_attempted`
    /// when no matching implementation symbol exists.
    #[arg(long = "spec-only-on-missing", default_value_t = false)]
    pub spec_only_on_missing: bool,
}
