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
# Default suite (all stable cases — everything except known-UNKNOWN research tags).
pwsh tests/e2e/Run-E2ETests.ps1

# Just one tag.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc

# Multiple tags.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc,rust_havoc,bounded_loop

# Show what would run.
pwsh tests/e2e/Run-E2ETests.ps1 -List

# Include the research-only cases (e.g. box_allocator).
pwsh tests/e2e/Run-E2ETests.ps1 -All

# Exclude a specific tag (combinable with -All).
pwsh tests/e2e/Run-E2ETests.ps1 -All -Exclude box_allocator

# Opt out entirely (used by the pre-commit hook).
$env:SKIP_SAW_TESTS = '1'
```

## Tags

All tags except `box_allocator` run by default (no `-Tag`/`-All` needed).
Adding a new tag to `cases.psd1` automatically gates it in CI.

| Tag                  | Cases | CI default | What it covers                                                                                  |
|----------------------|------:|:----------:|-------------------------------------------------------------------------------------------------|
| `cpp_havoc`          | 22 | ✓ | `tests/e2e/cases/02-havoc-coverage/**/*.cpp` — vtable + global havoc spec tests (C++). |
| `rust_havoc`         | 11 | ✓ | `tests/e2e/cases/02-havoc-coverage/**/*.rs` + `tests/e2e/cases/03-rust-trait-dispatch/` — Rust havoc and statically-resolvable trait dispatch. |
| `bounded_loop`       |  2 | ✓ | `tests/e2e/cases/01-tutorial/bounded_loop/**` — bit-level ripple-carry and bounded data-dependent loops. |
| `rust_equiv`         |  5 | ✓ | `tests/e2e/cases/04-cpp-rust-equivalence/**` — C++/Rust equivalence via shared Cryptol spec. |
| `string_ops`         |  2 | ✓ | `tests/e2e/cases/05-string-ops/has_null_byte/**` — SWAR null-byte detection (C++). |
| `strings`            |  2 | ✓ | `tests/e2e/cases/05-string-ops/count_digits/**` — C-string `count_digits` over `_In_reads_(8) const char*` (C++). |
| `cryptol_len_bind`   |  2 | ✓ | `tests/e2e/cases/05-string-ops/cryptol_len_bind/**` — ArrayView rule 1: buffer size derived automatically from a length-polymorphic Cryptol signature. |
| `int_ops`            | 16 | ✓ | `tests/e2e/cases/06-int-ops/**` — integer-op coverage fillers (multi-arg signed min, predicate bit-trick, byte swap, u8 popcount). |
| `cpp_throws`         |  6 | ✓ | `tests/e2e/cases/06-throws-exception/**` — C++ throw/catch exception handling. |
| `csep590b_c04`       | 16 | ✓ | `tests/e2e/cases/01-tutorial/csep590b_c04/**` — course tutorial cases (clamp_sub, safe_mul, count_groups, make_change, isqrt). |
| `string_content`     |  4 | ✓ | `tests/e2e/cases/05-string-ops/string_content/**` — string content pre/post-condition coverage. |
| `rust_adversarial`   |  5 | ✓ | `tests/e2e/cases/99-research/rust_adversarial/**` — research cases for known verifier blind spots. |
| `stl_coverage`       | 29 | ✓ | `tests/e2e/cases/10-stl-coverage/**` — `<algorithm>`/`<numeric>`/`<string>`/`<vector>`/`unique_ptr` STL havoc + override cases. |
| `cpp_overrides`      | 13 | ✓ | `tests/e2e/cases/08-overrides/**` — bitcode extern overrides (bump allocator, MSVC mutex helpers, variadic clobber, etc.). |
| `cpp_stateful`       | 12 | ✓ | `tests/e2e/cases/09-stateful/**` — stateful-method whole-object post-state via `--out-buffer-param`/`--cryptol-fn-out`. |
| `rust_parity`        |  4 | ✓ | `tests/e2e/cases/11-rust-parity/**` — custom-script Rust parity checks (spec-only-on-missing, variant-map, return narrowing). |
| `aggregate_bridge`   |  3 | ✓ | Rust↔C++ aggregate bridge (packed tuple and sret struct via `StructValue`). |
| `enum_constraints`   |  3 | ✓ | `tests/e2e/cases/07-enum-constraints/**` — enum discriminant range/membership preconditions. |
| `rust_cleanup`       |  3 | ✓ | `tests/e2e/cases/11-rust-parity/**` — Rust cleanup-funclet lowering (`cleanuppad`/`cleanupret`). |
| `struct_shape`       |  3 | ✓ | `tests/e2e/cases/05-string-ops/struct_shape_recognizer/**` — `(T* buf, size_t len)` struct-shape recognizer. |
| `in_z_macro`         |  2 | ✓ | `tests/e2e/cases/05-string-ops/in_z_macro/**` — `_In_z_(N)` SAL macro (null-terminator-bounded buffers). |
| `proof_markers`      |  1 | ✓ | Proof-marker output validation (schema-1 `result.json` shape). |
| `sret_slice`         |  1 | ✓ | `tests/e2e/cases/09-type-coverage/sret_slice` — sret pre-state slice (`take`/`drop` emission). |
| `box_allocator`      |  1 | ✗ | `tests/e2e/cases/99-research/box_allocator` — **research only**; produces `UNKNOWN` under the current pipeline. Excluded by default; use `-All` to include. |

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

The new tag runs in CI automatically — no further edits needed. Only
add a tag to `$researchTags` in `Run-E2ETests.ps1` if the case is
intentionally expected to produce `UNKNOWN` under the current pipeline.

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
