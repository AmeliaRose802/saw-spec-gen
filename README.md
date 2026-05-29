# saw-spec-gen

End-to-end formal verification pipeline that proves a C++ or Rust function
matches a hand-written [Cryptol](https://cryptol.net/) spec — or proves
two implementations equivalent — using
[SAW](https://saw.galois.com/).

You write the spec once. The wrapper scripts compile your source,
auto-generate the SAW override scaffolding from the compiler-provided
type info, and hand the whole thing to SAW. You never write a `.saw`
file by hand.

```powershell
./verify.ps1 -CppFile add_one.cpp `
             -CryptolSpec add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one
# → RESULT: VERIFIED by z3
```

## What's in the box

- **`verify.ps1`** — prove a C++ function matches a Cryptol spec.
  `clang → bitcode → AST → saw-spec-gen → SAW`.
- **`verify-rust.ps1`** — same, for a Rust function. `rustc → bitcode →
  resolve mangled symbol → SAW`. The Rust source needs **no**
  `#[no_mangle]` or `pub extern "C"`.
- **`verify-equiv.ps1`** — prove a C++ function and a Rust function both
  match the same Cryptol spec (and therefore each other). Reports
  `EQUIVALENT` / `NOT EQUIVALENT` and shows the counterexample side-by-side
  when they disagree.
- **`saw-spec-gen`** — the Rust CLI underneath. Reads
  clang `-ast-dump=json` / mir-json / `.ll` and emits adversarial SAW
  override specs that model external functions as **as-pessimistic-as-the-type-system-allows**:
  mutable memory is havoced, const memory is preserved.

The verify scripts work on Windows, Linux and macOS through a shared
discovery layer (`scripts/discover-tools.ps1`) — see
[Installation](#installation).

## Quick start

```powershell
# Clone + run the one-shot installer (downloads clang, SAW, z3).
git clone https://github.com/AmeliaRose802/saw-spec-gen
cd saw-spec-gen
pwsh scripts/init.ps1            # or:  bash scripts/init.sh

# Verify a C++ function against a Cryptol spec.
./verify.ps1 `
    -CppFile     demo/bounded_loop/add_one.cpp `
    -CryptolSpec demo/bounded_loop/add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one

# Same for Rust.
./verify-rust.ps1 `
    -RustFile    demo/bounded_loop/add_one.rs `
    -CryptolSpec demo/bounded_loop/add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one

# Prove both implementations match the same spec (and so each other).
./verify-equiv.ps1 `
    -CppFile     demo/rust_equalivence_demo/nothing_sketchy/add_one_verified.cpp `
    -RustFile    demo/rust_equalivence_demo/nothing_sketchy/add_one_verified.rs `
    -CryptolSpec demo/rust_equalivence_demo/nothing_sketchy/add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one
```

The output directory (`out_<basename>/`) contains every intermediate file
— the `.bc`, the AST JSON, every generated override spec, the
`verify.saw` script, and a structured `result.json` machine-readable
verdict.

## The verification model

For each function you don't (or can't) verify directly — interface
methods, OS calls, allocators, callbacks — SAW needs an *override
spec* that says what the function is allowed to do. Writing these by
hand is error-prone:

- **Too loose**: fresh return value, but forget to havoc mutable
  memory → SAW misses bugs where the callee corrupts state.
- **Too tight**: hardcoded return values → SAW misses bugs that
  only appear with certain return values.
- **Wrong shape**: forget `const`, miscount struct fields, wrong LLVM
  type → spec silently doesn't match.

`saw-spec-gen` derives the **maximally adversarial** spec consistent
with the type system, SAL annotations, and compiler attributes.

| Constraint source | Treated as |
|---|---|
| `const T*` / `const T&` | **Preserved** — `llvm_alloc_readonly`, postcondition asserts unchanged |
| `_In_` / `_In_reads_(n)` | **Preserved** — SAL says input-only |
| Non-const `T*` / `T&` / non-const `this` | **Havoced** — every reachable byte gets a fresh symbolic after the call |
| `_Out_` / `_Out_writes_(n)` / `_Inout_` | **Havoced** |
| Return value | **Unconstrained** — solver picks any value of the right type |
| Conflict (`const` + `_Inout_`) | **Strictest wins** (preserved) |

Struct fields with known layout are decomposed: each scalar field gets
its own fresh var, each pointer field gets a separate allocation that
is individually havoced or preserved. STL types use known MSVC x64
sizes (`std::string`=32, `std::vector`=24, …).

The SAW verifier then proves your function is correct for **every
possible implementation** of every overridden function.

> STL functions are assumed not to be adversarial — safe assume-specs are
> auto-generated for them. A "STL might be evil" mode is on the
> roadmap.

## Installation

The `scripts/init.*` installers download and configure every external
tool under `~/.saw-spec-gen/`:

```powershell
pwsh scripts/init.ps1          # Windows
```

```bash
bash scripts/init.sh           # Linux / macOS
```

What they do:

1. Verify `rustc`/`cargo` are on PATH (prompts to install via
   [rustup](https://rustup.rs) if not).
2. Run `cargo build --release` to produce the `saw-spec-gen` CLI.
3. Locate `clang` + `llvm-as`. On Windows, download the official LLVM
   tarball if missing. On Linux/macOS, print the package-manager
   command (`apt install clang llvm`, `brew install llvm`, …).
4. Download SAW + bundled solvers (z3 etc.) from the
   [GaloisInc/saw-script releases](https://github.com/GaloisInc/saw-script/releases).
5. Write `~/.saw-spec-gen/env.ps1` (and `env.sh` on Unix) which the
   verify scripts auto-discover on every run.

The installers are idempotent. Pass `-Force` (PowerShell) or `FORCE=1`
(bash) to redownload everything.

### Manual configuration

To point at tools installed elsewhere, set any of:

| Variable                       | What it points to                                       |
|--------------------------------|---------------------------------------------------------|
| `SAW_SPEC_GEN_LLVM_BIN`        | Directory containing `clang`, `llvm-as`, `llvm-dis`     |
| `SAW_SPEC_GEN_SAW`             | Full path to the `saw` binary                           |
| `SAW_SPEC_GEN_SOLVER_BIN`      | Directory containing `z3` (and optional yices/cvc4)     |
| `SAW_SPEC_GEN_RUSTC`           | Full path to `rustc` (defaults to `rustc` on PATH)      |
| `SAW_SPEC_GEN_LLVM_TARGET`     | Override the default LLVM target tuple                  |

These take precedence over `~/.saw-spec-gen/env.ps1`, which in turn
takes precedence over PATH lookup.

## CLI reference

The verify scripts orchestrate the `saw-spec-gen` CLI internally —
you only need these commands directly if you're embedding the tool
into a different pipeline.

```text
saw-spec-gen <SUBCOMMAND>
  from-clang-ast        Generate SAW specs from a clang AST dump (C/C++)
  from-mir-json         Generate SAW specs from mir-json output (Rust)
  from-llvm-ir          Generate SAW specs from LLVM IR text (any language)
  gen-verify            Generate a complete, runnable .saw file end-to-end
                        (specs + vtable stubs + Cryptol equivalence check)
  gen-rust-trait-stubs  Generate Rust trait vtable stubs + havoc specs
                        for `&dyn Trait` parameters
  filter-ast            Strip system-header decls from a clang AST dump
                        (used internally to keep AST size under control)
  patch-llvm-ir         Patch a .ll file so SAW 1.5 / Crucible can load it
                        (strips MSVC EH metadata, rewrites poison → undef)
  coverage              Report spec coverage for a verified codebase
```

`gen-verify` is the one the verify scripts use. Example:

```bash
saw-spec-gen gen-verify \
    --ast      ast.json \
    --bitcode  code.bc \
    --llvm-ir  code.ll \
    --cryptol-spec add_one_spec.cry \
    --cryptol-fn   add_one_spec \
    --function     add_one \
    --output       out_add_one/
```

`from-clang-ast` and friends are still available if you want raw
specs to drop into your own SAW script:

```bash
clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json
saw-spec-gen from-clang-ast --input ast.json --output specs/ --emit-stubs
```

That produces `vtable_stubs.ll`, `interface_overrides.saw`, and a
per-method `*_havoc_spec.saw` for every virtual method, ready to
`include` from your verification script.

## Constraint sources

| Source | What it tells us | How it's used |
|--------|-----------------|---------------|
| C++ `const T*` / Rust `&T` | Parameter is read-only | `llvm_alloc_readonly` + preservation postcondition |
| C++ `T*` / Rust `&mut T` | Parameter is mutable | `llvm_alloc` + havoced postcondition |
| SAL `_In_` / `_In_reads_(n)` | Input-only | Preserved; buffer sized to `n` |
| SAL `_Out_` / `_Out_writes_(n)` | Output-only | Havoced; buffer sized to `n` |
| SAL `_Inout_` | Read-write | Havoced (unless const overrides) |
| `noexcept` / `nounwind` | Cannot throw | No exception-path warning |
| Rust `enum` | Finite variants | Discriminant range constraint |
| Rust `Option<T>` / `Result<T,E>` | Two-variant enum | Discriminant ∈ {0, 1} |
| LLVM `sret` | Return via pointer | Modeled as output parameter |
| Struct fields | Known layout | Per-field decomposition in havoc specs |
| Struct size | Byte-level layout | `llvm_array N (llvm_int 8)` |
| STL types | Known MSVC x64 sizes | `std::string`=32, `std::vector`=24, etc. |

## Examples

Working end-to-end demos live in [demo/](demo/):

| Directory | What it shows |
|---|---|
| `bounded_loop/` | The hello-world: prove `add_one`/`sum_first_n` in C++ and Rust against the same Cryptol spec |
| `string_ops/` | C string handling with verified/disproved pairs |
| `strings/` | `std::string` and C-string equivalents, both verifying and counterexample-producing |
| `async_rust/` | Verify a Rust `async fn` by targeting its coroutine `resume` symbol |
| `rust_equalivence_demo/` | Cross-language equivalence — same logic in C++ and Rust |
| `rust_adversarial_holes/` | "Sneaky" Rust patterns (interior mutability, raw aliasing, drop side effects, unreachable_unchecked, …) — every disproved case shows the harness catching what unit tests would miss |
| `vtable_havoc_spec_demos/` | C++ virtual-dispatch verification: prove correctness for *any* implementation of an interface |
| `csep590b_c04/` | Course-problem suite (clamp_sub, safe_mul, count_groups, make_change, isqrt) |

Each directory's `out_*/verify.saw` is fully readable — useful for
learning what a hand-written SAW proof of the corresponding pattern
would look like.

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

The constraint engine (`src/constraints.rs`) is language-independent.
All three parsers produce the same `FunctionInfo` structs and the SAW
emitter doesn't know which language the input came from.

## Tests

```bash
# Rust crate tests.
cargo test --release

# Full end-to-end SAW demo regressions (auto-skips if SAW not installed).
pwsh tests/saw_demos/Run-SawDemos.ps1

# Filter by tag.
pwsh tests/saw_demos/Run-SawDemos.ps1 -Tag cpp_havoc,bounded_loop

# Dry-run.
pwsh tests/saw_demos/Run-SawDemos.ps1 -List
```

Add new demos by appending entries to
[tests/saw_demos/cases.psd1](tests/saw_demos/cases.psd1) — don't write
bespoke runner scripts.

The pre-commit hook (`git config core.hooksPath .githooks`) runs the
line-count check and the demo suite. Skip the SAW portion with
`SKIP_SAW_TESTS=1` for a fast amend.

## Project rules

See [.github/copilot-instructions.md](.github/copilot-instructions.md):

- **500 non-whitespace lines per source file**, enforced by
  `scripts/check-line-count.sh` (and the `line-count` CI job). Split
  files along clear seams (parser vs emitter, per-target backends,
  …) rather than appending past the limit.
- Don't add new entries to `.linecount-allow`. Shrink files in it
  when you touch them.

## License

MIT
