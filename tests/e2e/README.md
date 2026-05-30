# end-to-end test suite

End-to-end regression suite for every `tests/e2e/cases/` scenario. Each case runs
the full verification pipeline (compile → spec generation → SAW) and
asserts the verdict matches an expected `VERIFIED` / `DISPROVED` /
`EQUIVALENT` / `NOT EQUIVALENT` / `UNKNOWN`.

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

## Pre-commit integration

`.githooks/pre-commit` (activated with `git config core.hooksPath .githooks`)
runs:

1. `scripts/check-line-count.sh` — fast, always required.
2. `tests/e2e/Run-E2ETests.ps1` — skipped automatically when SAW is
   not installed, or when `SKIP_SAW_TESTS=1` is exported.

## Runner semantics

Each case writes nothing back into `tests/`. Output artifacts live next
to the test source (`tests/e2e/cases/<topic>/out_<name>/`) and are cleared at the
start of each case. A failing case writes its full captured stdout to
`tests/e2e/last-fail-<idx>.log` for triage.
