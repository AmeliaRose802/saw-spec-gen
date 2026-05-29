# SAW demo test suite

End-to-end regression suite for every `demos/` scenario. Each case runs
the full verification pipeline (compile → spec generation → SAW) and
asserts the verdict matches an expected `VERIFIED` / `DISPROVED` /
`EQUIVALENT` / `NOT EQUIVALENT` / `UNKNOWN`.

## Files

| File                    | Role                                                          |
|-------------------------|---------------------------------------------------------------|
| `cases.psd1`            | Declarative manifest — one entry per demo case.               |
| `Run-SawDemos.ps1`      | Runner: loads the manifest, dispatches to `verify*.ps1`.      |
| `last-fail-<N>.log`     | Auto-saved on failure (full stdout of the failing case).      |

## Running locally

```powershell
# Default suite (everything except known-UNKNOWN research cases).
pwsh tests/saw_demos/Run-SawDemos.ps1

# Just one tag.
pwsh tests/saw_demos/Run-SawDemos.ps1 -Tag cpp_havoc

# Multiple tags.
pwsh tests/saw_demos/Run-SawDemos.ps1 -Tag cpp_havoc,rust_havoc,bounded_loop

# Show what would run.
pwsh tests/saw_demos/Run-SawDemos.ps1 -List

# Include the research-only cases (e.g. box_allocator).
pwsh tests/saw_demos/Run-SawDemos.ps1 -All

# Opt out entirely (used by the pre-commit hook).
$env:SKIP_SAW_TESTS = '1'
```

## Tags

| Tag                  | What it covers                                                                                  |
|----------------------|-------------------------------------------------------------------------------------------------|
| `cpp_havoc`          | `demos/02-havoc-coverage/**/*.cpp` — vtable + global havoc spec demos (C++).                    |
| `rust_havoc`         | `demos/02-havoc-coverage/**/*.rs` + `demos/03-rust-trait-dispatch/{static,dynamic,external}/*.rs` — Rust havoc and statically-resolvable trait dispatch. |
| `bounded_loop`       | `demos/01-tutorial/bounded_loop/**` — bit-level ripple-carry and bounded data-dependent loops.  |
| `rust_equiv`         | `demos/04-cpp-rust-equivalence/**` — C++/Rust equivalence via shared Cryptol spec.              |
| `trait_unknown_impl` | `demos/03-rust-trait-dispatch/unknown_impl` — opaque `&dyn Trait` impl, vtable stubs.           |
| `string_ops`         | `demos/05-string-ops/has_null_byte/**` — SWAR null-byte detection (C++).                        |
| `strings`            | `demos/05-string-ops/count_digits/**` — C-string + `std::string` `count_digits` (C++).          |
| `async_rust`         | `demos/06-async-rust/add_one_coroutine` — `async fn` coroutine lowering, proves resume == spec. |
| `rust_adversarial`   | `demos/99-research/rust_adversarial/**` — research cases for known verifier blind spots.  |
| `box_allocator`      | `demos/99-research/box_allocator` — excluded by default; produces `UNKNOWN` under the current pipeline. |

## Adding a new case

1. Drop the source + Cryptol spec into a new `demos/<topic>/` directory.
2. Append an entry to `cases.psd1`:

   ```powershell
   @{ Tag = 'rust_havoc'; Runner = 'rust'
      Dir = 'demos/my_new_case'; File = 'my_fn.rs'
      Expected = 'VERIFIED' }
   ```

   Convention defaults (`Cry = add_one_spec.cry`, `CryptolFn = add_one_spec`,
   `Function = add_one`) cover the common case; override per-key when your
   demo uses different names.
3. Run `pwsh tests/saw_demos/Run-SawDemos.ps1 -Tag <your tag>` to confirm
   it goes green.

## Pre-commit integration

`.githooks/pre-commit` (activated with `git config core.hooksPath .githooks`)
runs:

1. `scripts/check-line-count.sh` — fast, always required.
2. `tests/saw_demos/Run-SawDemos.ps1` — skipped automatically when SAW is
   not installed, or when `SKIP_SAW_TESTS=1` is exported.

## Runner semantics

Each case writes nothing back into `tests/`. Output artifacts live next
to the demo source (`demos/<topic>/out_<name>/`) and are cleared at the
start of each case. A failing case writes its full captured stdout to
`tests/saw_demos/last-fail-<idx>.log` for triage.
