# saw-spec-gen

End-to-end formal verification pipeline that proves a C++ or Rust
function matches a hand-written [Cryptol](https://cryptol.net/) spec —
or proves two implementations equivalent — using
[SAW](https://saw.galois.com/). Primary use case: validating Rust ports
of legacy C++ code.

You write the Cryptol spec once. The wrapper scripts compile your
source, auto-generate the SAW override scaffolding from the compiler's
own type info, and hand it all to SAW. You never write a `.saw` file
by hand.

```powershell
pwsh -File ./verify.ps1 -CppFile tests/e2e/cases/01-tutorial/bounded_loop/add_one_verified.cpp `
                       -CryptolSpec tests/e2e/cases/01-tutorial/bounded_loop/add_one_spec.cry `
                       -CryptolFn add_one_spec -Function add_one
# → RESULT: VERIFIED by z3
```

## What's in the box

- **`saw-spec-gen verify-cpp`** — native C++ verification pipeline.
  `clang → bitcode → AST → gen-verify → SAW`.
- **`verify.ps1`** — thin PowerShell shim over `saw-spec-gen verify-cpp`
  for existing callers.
- **`saw-spec-gen verify-rust`** — native Rust verification pipeline:
  `rustc → bitcode → llvm-dis → gen-verify-rust → SAW`, with
  DISPROVED counterexample replay + `result.json` output. No
  `#[no_mangle]` / `pub extern "C"` required.
- **`verify-rust.ps1`** — thin PowerShell shim over `saw-spec-gen
  verify-rust` for existing callers.
- **`verify-equiv.ps1`** — prove a C++ function and a Rust function
  both match the same Cryptol spec (and therefore each other). Reports
  `EQUIVALENT` / `NOT EQUIVALENT` with a side-by-side counterexample
  on disagreement.
- **`verify-all.ps1`** — batch driver. Consumes a JSON manifest of
  Cryptol function names (e.g. from `pretty-specs --emit-function-list`),
  discovers matching `<name>.cpp` / `<name>.rs` files, dispatches
  per-function, and writes one `result.json` per run for
  `collect-results` to roll up.
- **`saw-spec-gen`** — the Rust CLI underneath. Reads clang
  `-ast-dump=json` / mir-json / `.ll` and emits adversarial SAW
  override specs: mutable memory havoced, const memory preserved,
  return values unconstrained.

All four scripts work on Windows, Linux, and macOS through a shared
discovery layer (`scripts/discover-tools.ps1`).

On Windows, use PowerShell 7 (`pwsh`), not the built-in Windows
PowerShell 5.1. If `pwsh` is missing, install it with:

```powershell
winget install --id Microsoft.PowerShell --source winget
```

Rust 1.85 or newer is required. Older Cargo versions cannot parse some
dependency manifests that use Rust 2024 edition metadata. Update with:

```powershell
rustup update stable
```

## Quick start

```powershell
git clone https://github.com/AmeliaRose802/saw-spec-gen
cd saw-spec-gen
pwsh -File scripts/init.ps1      # or: bash scripts/init.sh — downloads clang, SAW, z3

pwsh -File ./verify.ps1 -CppFile tests/e2e/cases/01-tutorial/bounded_loop/add_one_verified.cpp `
                       -CryptolSpec tests/e2e/cases/01-tutorial/bounded_loop/add_one_spec.cry `
                       -CryptolFn add_one_spec -Function add_one
```

For Rust, either call `saw-spec-gen verify-rust` directly or keep using
`verify-rust.ps1` (which now forwards to the native subcommand).
`verify-equiv.ps1` remains the C++/Rust equivalence entrypoint.

The output directory (`out_<basename>/`) contains every intermediate
file — the `.bc`, the AST JSON, every generated override spec, the
`verify.saw` script, and a structured `result.json`
([schema](docs/result-json.md)).

## The verification model

For every external function — interface methods, OS calls, allocators,
callbacks — SAW needs an *override spec* describing what the callee
may do. `saw-spec-gen` derives the **maximally adversarial** spec the
type system, SAL annotations, and compiler attributes allow:

| Constraint source                       | Treated as |
|-----------------------------------------|------------|
| `const T*` / `const T&` / Rust `&T`     | **Preserved** — `llvm_alloc_readonly`, postcondition asserts unchanged |
| `_In_` / `_In_reads_(n)`                | **Preserved**; buffer sized to `n` |
| Non-const `T*` / `T&` / `&mut T` / non-const `this` | **Havoced** — every reachable byte becomes fresh symbolic |
| `_Out_` / `_Out_writes_(n)` / `_Inout_` | **Havoced**; buffer sized when annotated |
| Return value                            | **Unconstrained** — solver picks any value of the right type |
| LLVM `sret`                             | Modeled as an output parameter |
| `noexcept` / `nounwind`                 | Suppresses the exception-path warning |
| Rust `enum` / `Option<T>` / `Result<T,E>` | Discriminant range constraint (e.g. `∈ {0, 1}`) |
| Conflict (`const` + `_Inout_`)          | **Strictest wins** (preserved) |

Struct fields with known layout are decomposed per-field; STL types
use known MSVC x64 sizes (`std::string`=32, `std::vector`=24, …). SAW
then proves your function correct against **every possible** behaviour
of every overridden callee.

> STL functions are assumed non-adversarial — safe assume-specs are
> auto-generated for them. A "STL might be evil" mode is on the
> roadmap.

## Installation

`scripts/init.ps1` (Windows) / `scripts/init.sh` (Linux/macOS) download
and configure every external tool under `~/.saw-spec-gen/`:

Windows users must run the PowerShell scripts with PowerShell 7
(`pwsh`). Windows PowerShell 5.1 can mis-handle native command stderr
from tools such as Cargo and fail during setup. Rust 1.85 or newer is
required; run `rustup update stable` if `cargo --version` reports an
older release.

1. Verify `rustc`/`cargo` are on PATH (prompts for [rustup](https://rustup.rs)).
2. `cargo build --release` to produce the `saw-spec-gen` CLI.
3. Locate `clang` + `llvm-as`; on Windows, download the official LLVM
   tarball if missing.
4. Download SAW + bundled solvers (z3 etc.) from the
   [GaloisInc/saw-script releases](https://github.com/GaloisInc/saw-script/releases).
5. Write `~/.saw-spec-gen/env.{ps1,sh}` for the verify scripts to
   auto-discover.

Idempotent. Pass `-Force` (PowerShell) or `FORCE=1` (bash) to
redownload.

### Manual configuration

| Variable                       | What it points to                                       |
|--------------------------------|---------------------------------------------------------|
| `SAW_SPEC_GEN_LLVM_BIN`        | Directory containing `clang`, `llvm-as`, `llvm-dis`     |
| `SAW_SPEC_GEN_SAW`             | Full path to the `saw` binary                           |
| `SAW_SPEC_GEN_SOLVER_BIN`      | Directory containing `z3` (and optional yices/cvc4)     |
| `SAW_SPEC_GEN_RUSTC`           | Full path to `rustc` (defaults to PATH lookup)          |
| `SAW_SPEC_GEN_LLVM_TARGET`     | Override the default LLVM target tuple                  |

These take precedence over `~/.saw-spec-gen/env.{ps1,sh}`, which in
turn takes precedence over PATH lookup.

## CLI reference

The verify scripts orchestrate `saw-spec-gen` internally; use these
directly only when embedding into a different pipeline.

```text
saw-spec-gen <SUBCOMMAND>
  from-clang-ast        SAW specs from a clang AST dump (C/C++)
  from-mir-json         SAW specs from mir-json output (Rust)
  from-llvm-ir          SAW specs from LLVM IR text (any language)
  gen-verify            Complete runnable .saw file end-to-end
                        (specs + vtable stubs + Cryptol equivalence)
  gen-rust-trait-stubs  Rust trait vtable stubs + havoc specs for
                        `&dyn Trait` parameters
  filter-ast            Strip system-header decls from a clang AST dump
  patch-llvm-ir         Patch a .ll so SAW 1.5 / Crucible can load it
                        (strips MSVC EH metadata, poison → undef)
  collect-results       Aggregate per-run `result.json` files into
                        one `proof_manifest.json`
  dump-types            Serialize per-function parameter / return
                        types and struct layouts to `types.json`
```

`gen-verify` is the one the wrapper scripts use:

```bash
saw-spec-gen gen-verify --ast ast.json --bitcode code.bc --llvm-ir code.ll \
    --cryptol-spec add_one_spec.cry --cryptol-fn add_one_spec \
    --function add_one --output out_add_one/
```

`from-clang-ast` (and friends) emit raw specs you can `include` from
your own SAW script:

```bash
clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json
saw-spec-gen from-clang-ast --input ast.json --output specs/ --emit-stubs
```

That produces `vtable_stubs.ll`, `interface_overrides.saw`, and a
per-method `*_havoc_spec.saw` for every virtual method.

## Examples

End-to-end cases live in [tests/e2e/cases/](tests/e2e/cases/):

| Directory | What it shows |
|---|---|
| `01-tutorial/bounded_loop/` | Hello-world: `add_one`/`sum_first_n` in C++ and Rust against one Cryptol spec |
| `01-tutorial/csep590b_c04/` | Course problem set (clamp_sub, safe_mul, count_groups, make_change, isqrt) — buggy + fixed variants in both languages |
| `02-havoc-coverage/` | One folder per hazard scenario (`pointer_aliasing`, `class_member_clobbered`, `throws_exception`, `ctor_stub_false_verdicts`, …); each holds the C++ and Rust `_verified`/`_disproved` pair |
| `03-rust-trait-dispatch/` | Rust-only: `static_dispatch`, `dynamic_dispatch`, `external_crate` |
| `04-cpp-rust-equivalence/` | Cross-language equivalence (`compute_fee_reordered`, `sat_add_optimized`, `not_operator_trap`) |
| `05-string-ops/` | `has_null_byte` SWAR detection; `count_digits` over `_In_reads_(8) const char*` |
| `99-research/rust_adversarial/` | "Sneaky" Rust (interior mutability, raw aliasing, drop side effects, `unreachable_unchecked`, …) — every disproved case shows the harness catching what unit tests would miss |
| `99-research/box_allocator/` | Known-`UNKNOWN`: `Box::new` path the front-end can't model yet |

Each scenario's `out_*/verify.saw` is readable — a useful template for
hand-written SAW proofs of the same pattern.

## Architecture

```
Input                   Constraint Engine         Output
─────                   ─────────────────         ──────
clang AST  (C/C++)        FunctionInfo              .saw specs
mir-json   (Rust)         ParamInfo                 vtable_stubs.ll
LLVM IR    (any)          TypeInfo                  havoc specs
                          Mutability                interface_overrides.saw
                          derive_constraints()      .cry predicates
```

`src/constraints.rs` is language-independent. All three parsers
produce the same `FunctionInfo` structs and the SAW emitter doesn't
know which language the input came from.

## Tests

```bash
cargo test --release                          # crate tests
pwsh tests/e2e/Run-E2ETests.ps1               # full SAW suite (auto-skips if SAW absent)
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc,bounded_loop
pwsh tests/e2e/Run-E2ETests.ps1 -List         # dry run
```

Add cases by appending entries to
[tests/e2e/cases.psd1](tests/e2e/cases.psd1) — see
[tests/e2e/README.md](tests/e2e/README.md) for the manifest schema and
tag list. Don't write bespoke runner scripts.

## Project rules

See [.github/copilot-instructions.md](.github/copilot-instructions.md).
The big one: **500 non-whitespace lines per source file**, enforced by
`scripts/check-line-count.sh` and the `line-count` CI job. Split along
clear seams (parser vs emitter, per-target backends, …) rather than
appending past the limit. Don't add new entries to `.linecount-allow`;
shrink files in it when you touch them.

## License

MIT
