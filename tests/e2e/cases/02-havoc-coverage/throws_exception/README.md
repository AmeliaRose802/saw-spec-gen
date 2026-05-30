# Throws an exception

Four `add_one` variants that demonstrate what happens when C++ source
uses `throw` / `try` / `catch`. All target the same Cryptol spec
[`add_one_spec.cry`](add_one_spec.cry):

```cryptol
add_one_spec : [32] -> [32]
add_one_spec x = x + 1
```

| File | What it does | SAW RESULT |
|------|--------------|------------|
| [`add_one_verified.cpp`](add_one_verified.cpp) | plain `return x + 1` (no exceptions) | **VERIFIED** by z3 |
| [`add_one_disproved.cpp`](add_one_disproved.cpp) | `if (x == 42) throw 1; return x + 1;` | **DISPROVED** with counterexample `x = 42` |
| [`add_one_throws_caught_verified.cpp`](add_one_throws_caught_verified.cpp) | helper throws on `x == 42`, caller `catch(int)` swallows it and still returns `x + 1` | **VERIFIED** by z3 (requires exception-lower pass) |
| [`add_one_multi_catch_disproved.cpp`](add_one_multi_catch_disproved.cpp) | two custom exception types; harmless catch falls through to `x + 1`, harmful catch returns `0` | **DISPROVED** with counterexample `x = 99` (requires exception-lower pass) |

The last two cases only verify when the standalone
[`exception-lower`](https://github.com/AmeliaRose802/llvm-exception-lower)
LLVM pass is available; see
["Catching the throw"](#catching-the-throw-via-exception-lower) below.

## What clang emits for `throw`

On `x86_64-pc-windows-msvc`, clang lowers a C++ `throw` to the MSVC
ABI helper:

```llvm
call void @_CxxThrowException(ptr %ex, ptr @_TI1H)
unreachable
```

`@_TI1H` is a throw-info global that points at a catchable-type
array, which in turn points at a type descriptor. Those globals are
initialized with `ptrtoint`/`sub` constant expressions relative to
`@__ImageBase`, e.g.

```llvm
@"_CT??_R0H@84" = linkonce_odr unnamed_addr constant %eh.CatchableType {
    i32 1,
    i32 trunc (i64 sub nuw nsw
                  (i64 ptrtoint (ptr @"??_R0H@8"   to i64),
                   i64 ptrtoint (ptr @__ImageBase to i64)) to i32),
    ...
}, section ".xdata", comdat
```

That `ptrtoint`-difference pattern used to abort SAW's global-initializer
loader before symbolic execution even started:

```
Encountered error while processing global @"_CT??_R0H@84":
Illegal operation applied to pointer argument
```

That was upstream bug
[handoff_saw_msvc_eh_globals.md](../../../handoff_saw_msvc_eh_globals.md),
fixed in `crucible-llvm`'s `Constant.hs` by treating
`ptrtoint(@A) - ptrtoint(@B)` on distinct symbols as
`UndefConst (IntType n)` (and propagating that undef through `trunc`/
`ZExt`/`SExt`). The metadata fields are never read by user code — only
by the OS unwinder — so an undef representation is sound and lets
verification proceed.

## What SAW now proves

With the fix in place:

1. SAW loads the bitcode cleanly. The `.xdata` metadata globals
   become opaque integers with undef contents.
2. `gen-verify` already emits an auto-spec for the external
   `_CxxThrowException` under `specs_experimental/` — it lets the
   call return normally with havoced memory.
3. The `unreachable` instruction that clang places right after the
   `_CxxThrowException` call prunes the throw path: SAW symbolically
   executes through the non-throwing branch only.
4. The function effectively models as
   *"if `add_one(x)` returns, then it returns `x + 1`"* — i.e.
   partial correctness.

The Cryptol spec [`add_one_spec.cry`](add_one_spec.cry) is a *total*
function (`add_one_spec x = x + 1` for every input). Equivalence to a
total spec is a strictly stronger property: it requires that the
C++ function return on every input. `add_one_disproved.cpp` does *not*
return on `x == 42` — it throws — so equivalence to the total
Cryptol spec must fail, and it does, with exactly that counterexample.

This is the right outcome:

- It catches the discrepancy between the C++ implementation
  (partial: throws on `x == 42`) and the intended mathematical
  contract (total: defined on every `x`).
- It avoids a *false* VERIFIED that would have been the worst possible
  outcome — pretending a throwing function satisfies a total spec
  silently is exactly the bug verification is supposed to find.

If the intent of the C++ code is genuinely "this never throws on
valid inputs," the Cryptol spec should encode the precondition (e.g.
exclude `x == 42` from the input set) or model the result type as
`Option`/`Result`. SAW will then re-prove the equivalence under that
restricted contract.

## Pedagogical takeaway

C++ exceptions interact with SAW exactly the way `unreachable` does:
throw paths are *pruned*, not modeled. SAW proves the
partial-correctness property "if the function returns, it satisfies
the spec." Whether that matches your intent depends on whether your
Cryptol spec is partial (precondition-aware) or total.

The pipeline does not need the
[`saw-tools/exception-lower`](https://github.com/GaloisInc/saw-script/tree/master/saw-tools/exception-lower)
pre-pass for this MSVC case — that tool targets the Itanium EH ABI
(`__cxa_throw` / `invoke` / `landingpad`) and is still the right
choice for Linux/macOS or `*-pc-windows-gnu` builds. On MSVC, the
in-Crucible fix is sufficient.

## Catching the throw via exception-lower

A propagating `throw` (above) lowers cleanly because clang emits
`call _CxxThrowException` + `unreachable` — no funclet records and
SAW just prunes the throw path. The moment a matching `catch`
appears, clang emits MSVC SEH funclet plumbing
(`catchswitch` / `catchpad` / `catchret`) which `llvm-pretty-bc-parser`
rejects (`FUNC_CODE_CATCHSWITCH`).

[`add_one_throws_caught_verified.cpp`](add_one_throws_caught_verified.cpp) and
[`add_one_multi_catch_disproved.cpp`](add_one_multi_catch_disproved.cpp) both depend on
funclet IR. `verify.ps1` runs the standalone
[`exception-lower`](https://github.com/AmeliaRose802/llvm-exception-lower)
pass on the freshly compiled bitcode before handing it to SAW; the
pass rewrites both Itanium (`invoke` / `landingpad` / `__cxa_throw`)
and MSVC SEH (`catchswitch` / `catchpad` / `_CxxThrowException`)
constructs into explicit error-flag CFG that the existing parser
accepts.

Once lowered:

- `throws_caught` — the catch arm clears the error flag and falls
  through to `return x + 1`, so SAW proves equivalence to the total
  Cryptol spec for every input. **VERIFIED**.
- `multi_catch` — the harmless catch arm clears the flag and falls
  through to `return x + 1`; the harmful arm returns `0` instead.
  SAW symbolically executes both arms and surfaces the harmful arm
  as a counterexample. **DISPROVED** at `x = 99`.

If the binary is not installed, `verify.ps1` prints a one-line note
and continues with text-only MSVC EH stripping; the funclet-bearing
demos will then fail to load with `FUNC_CODE_CATCHSWITCH`. Set
`SAW_SPEC_GEN_EXCEPTION_LOWER` (or put `exception-lower` / `exception-lower.exe`
on `PATH`, or drop it under `~/.saw-spec-gen/exception-lower/bin/`)
to enable the pipeline.

On first need, `verify.ps1` will try to auto-install the pass for you:

1. Download a prebuilt binary for your host platform from
   [llvm-exception-lower releases](https://github.com/AmeliaRose802/llvm-exception-lower/releases)
   into `~/.saw-spec-gen/exception-lower/bin/`. Currently published:
   `windows-x64` (`.zip`), `linux-x64` (`.tar.gz`). Other platforms
   fall through to step 2.
2. Clone + cmake-build from source if `cmake` and `git` are on `PATH`.

If neither path works, verification falls back gracefully and tells you
what to install. You can also run the installer explicitly:

```powershell
pwsh scripts/install-exception-lower.ps1
```

```bash
bash scripts/install-exception-lower.sh
```

## Running the tests

```powershell
# Control case — VERIFIED
./verify.ps1 `
    -CppFile     tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_verified.cpp `
    -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one

# Throws case — DISPROVED ( with counterexample x = 42)
./verify.ps1 `
    -CppFile     tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_disproved.cpp `
    -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one

# Throws-caught case — VERIFIED (requires exception-lower)
./verify.ps1 `
    -CppFile     tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_throws_caught_verified.cpp `
    -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one

# Multi-catch case — DISPROVED at x = 99 (requires exception-lower)
./verify.ps1 `
    -CppFile     tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_multi_catch_disproved.cpp `
    -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
    -CryptolFn   add_one_spec `
    -Function    add_one
```

All four cases are wired into the consolidated end-to-end test suite
([`tests/e2e/cases.psd1`](../../../tests/e2e/cases.psd1),
runner: [`tests/e2e/Run-E2ETests.ps1`](../../../tests/e2e/Run-E2ETests.ps1))
as regression tests. If SAW ever regresses on any case, the
pre-commit hook and any CI invocation of the suite will notice.
