# end-to-end test suite

End-to-end regression suite for every `tests/e2e/cases/` scenario. Each case runs
the full verification pipeline (compile → spec generation → SAW) and
asserts the verdict matches an expected `VERIFIED` / `DISPROVED` /
`EQUIVALENT` / `NOT EQUIVALENT` / `UNKNOWN`.
Negative generation cases can instead expect `REJECTED` before proof begins;
this is a runner classification, not a SAW verdict.

## Files

| File                    | Role                                                          |
|-------------------------|---------------------------------------------------------------|
| `cases.psd1`            | Declarative manifest — one entry per test case.              |
| `Run-E2ETests.ps1`      | Runner: loads the manifest, dispatches to `verify*.ps1`.      |
| `last-fail-<N>.log`     | Auto-saved on failure (full stdout of the failing case).      |

## Running locally

```powershell
# Default suite (everything except known-UNKNOWN research cases).
pwsh tests/e2e/Run-E2ETests.ps1

# Just one tag.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc

# Multiple tags.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc,rust_havoc,bounded_loop

# Show what would run.
pwsh tests/e2e/Run-E2ETests.ps1 -List

# Include the research-only cases (e.g. box_allocator).
pwsh tests/e2e/Run-E2ETests.ps1 -All

# Opt out entirely (used by the pre-commit hook).
$env:SKIP_SAW_TESTS = '1'
```

## Tags

| Tag                  | What it covers                                                                                  |
|----------------------|-------------------------------------------------------------------------------------------------|
| `cpp_havoc`          | `tests/e2e/cases/02-havoc-coverage/**/*.cpp` — vtable + global havoc spec tests (C++).                    |
| `rust_havoc`         | `tests/e2e/cases/02-havoc-coverage/**/*.rs` + `tests/e2e/cases/03-rust-trait-dispatch/{static,dynamic,external}/*.rs` — Rust havoc and statically-resolvable trait dispatch. |
| `bounded_loop`       | `tests/e2e/cases/01-tutorial/bounded_loop/**` — bit-level ripple-carry and bounded data-dependent loops.  |
| `rust_equiv`         | `tests/e2e/cases/04-cpp-rust-equivalence/**` — C++/Rust equivalence via shared Cryptol spec.              |
| `string_ops`         | `tests/e2e/cases/05-string-ops/has_null_byte/**` — SWAR null-byte detection (C++).                        |
| `strings`            | `tests/e2e/cases/05-string-ops/count_digits/**` — C-string `count_digits` over `_In_reads_(8) const char*` (C++).          |
| `cryptol_len_bind`   | `tests/e2e/cases/05-string-ops/cryptol_len_bind/**` — ArrayView rule 1: buffer size derived automatically from a length-polymorphic Cryptol signature. |
| `int_ops`            | `tests/e2e/cases/06-int-ops/**` — integer-op coverage fillers (multi-arg signed min, predicate bit-trick, byte swap, u8 popcount). |
| `cpp_stateful`       | `tests/e2e/cases/09-stateful/**` — stateful-method whole-object post-state via out-buffer postconditions, including inferred mutable `this` receivers, byte buffers, typed wide fields (`i32`), and named heterogeneous structs with padding (`llvm_struct`).  |
| `aggregate_bridge`   | `tests/e2e/cases/12-aggregate-bridge/**` — aggregate/struct ABI bridge tests: packed tuple returns, sret byte-buffer allocation, niche-packed enum remaps, and sret sub-callee havoc specs (issue #68). |
| `object_layout`      | Compiler-derived C++ storage: nested POD/sret, multiple bases, pointer frames, selected unions/variants, enums, bitfields and real mutex/optional KeyStore code; includes pre-proof rejection cases. |
| `rust_adversarial`   | `tests/e2e/cases/99-research/rust_adversarial/**` — research cases for known verifier blind spots.  |
| `box_allocator`      | `tests/e2e/cases/99-research/box_allocator` — excluded by default; produces `UNKNOWN` under the current pipeline. |

## Adding a new case

1. Drop the source + Cryptol spec into a new `tests/e2e/cases/<topic>/` directory.
2. Append an entry to `cases.psd1`:

   ```powershell
   @{ Tag = 'rust_havoc'; Runner = 'rust'
      Dir = 'tests/e2e/cases/my_new_case'; File = 'my_fn.rs'
      Expected = 'VERIFIED' }
   ```

   Convention defaults (`Cry = add_one_spec.cry`, `CryptolFn = add_one_spec`,
   `Function = add_one`) cover the common case; override per-key when your
   test uses different names.
3. Run `pwsh tests/e2e/Run-E2ETests.ps1 -Tag <your tag>` to confirm
   it goes green.

**Runner policy:** use only built-in runners — `cpp`, `rust`, `equiv`.
Do **not** add `Runner = 'custom'` or `Script = ...` to `cases.psd1`.
If a built-in runner lacks a needed capability, extend the runner instead
of wrapping a custom script. CI enforces this via the `no-custom-runners`
job; run `bash scripts/check-no-custom-runners.sh` locally to check.

### Compiler-layout case options

Use the built-in `cpp` runner; no custom script or runner is needed.
The current 19-case `object_layout` group has been validated on Windows and
Linux. Run it with:

```powershell
pwsh tests/e2e/Run-E2ETests.ps1 -Tag object_layout
```

| Manifest key | Behavior |
|---|---|
| `CxxStandard` | Forwards the language standard, e.g. `c++17`, to C++ verification. |
| `WindowsConfig`, `LinuxConfig` | Selects a platform-specific config relative to `Dir`, falling back to `Config`. The runner uses `LinuxConfig` on non-Windows hosts. |
| `ExpectedError` | Regex for an expected diagnostic. Classifies output as `REJECTED` only when it has no recognized `RESULT:` verdict, matches the regex and contains no `BEGIN_PROOF`. Pair with `Expected = 'REJECTED'`; an exception is not a rejection pass. |
| `LayoutRegions` | Checks top-level `memory_layout` schema `1`, platform ABI, and each named object's positive size/alignment and empty `unresolved` list. For `return`, requires sret lowering and asserted-field count equal to semantic-field count. Also requires a typed aligned allocation in the generated proof. |
| `ForbiddenOverrides` | With `LayoutRegions`, rejects generated `llvm_unsafe_assume_spec m` bindings whose symbols contain any listed literal substring. |

KeyStore checks `LayoutRegions = @('this','newKey','return')` and forbids
`_Mutex_base@std`/`scoped_lock` overrides, while keeping low-level runtime
assumptions explicit. Its DISPROVED twin has a correct return but corrupts
receiver state. See the [compiler-layout guide](../../docs/29-compiler-derived-object-layouts.md#L1).

The two legacy `partial_sret` cases under `aggregate_bridge` now intentionally
expect `REJECTED`: their prefix omits a real `tail` array, not compiler padding.
The expected diagnostic is `omits semantic field return.tail`, before proof.

## Pre-commit integration

`.githooks/pre-commit` (activated with `git config core.hooksPath .githooks`)
runs:

1. `scripts/check-line-count.sh` — fast, always required.
2. `scripts/check-no-custom-runners.sh` — fast, always required; rejects custom E2E runners.
3. `tests/e2e/Run-E2ETests.ps1` — skipped automatically when SAW is
   not installed, or when `SKIP_SAW_TESTS=1` is exported.

## Runner semantics

Each case writes nothing back into `tests/`. Output artifacts live next
to the test source (`tests/e2e/cases/<topic>/out_<name>/`) and are cleared at the
start of each case. A failing case writes its full captured stdout to
`tests/e2e/last-fail-<idx>.log` for triage.
