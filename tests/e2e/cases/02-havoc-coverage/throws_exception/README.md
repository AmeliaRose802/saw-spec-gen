# Throws an exception

Two `add_one` variants showing how SAW handles C++ `throw`. Both
target [`add_one_spec.cry`](add_one_spec.cry):

```cryptol
add_one_spec : [32] -> [32]
add_one_spec x = x + 1
```

| File | What it does | RESULT |
|------|--------------|--------|
| [`add_one_verified.cpp`](add_one_verified.cpp) | plain `return x + 1` | **VERIFIED** by z3 |
| [`add_one_disproved.cpp`](add_one_disproved.cpp) | `if (x == 42) throw 1; return x + 1;` | **DISPROVED** with counterexample `x = 42` |

## How throw paths are handled

On `x86_64-pc-windows-msvc`, clang lowers `throw` to
`call void @_CxxThrowException(...)` followed by `unreachable`. The
unwinder metadata it emits (`@_TI1H`, `@_CT??_R0H@84`, …) uses
`ptrtoint`/`sub` constant expressions against `@__ImageBase` that
SAW's global-initializer loader used to abort on:

```
Encountered error while processing global @"_CT??_R0H@84":
Illegal operation applied to pointer argument
```

The upstream fix in `crucible-llvm`'s `Constant.hs` treats
`ptrtoint(@A) - ptrtoint(@B)` on distinct symbols as `UndefConst`
(propagating through `trunc`/`zext`/`sext`). The fields are read only
by the OS unwinder, never by user code, so undef is sound.

With that fix in place SAW loads the bitcode cleanly, `gen-verify`'s
auto-spec lets `_CxxThrowException` return normally with havoced
memory, and the `unreachable` after the throw call prunes the throw
path — equivalent to *"if `add_one(x)` returns, it returns `x + 1`"*
(partial correctness).

`add_one_spec` is total (defined on every `x`). Equivalence to a
total spec requires the implementation to return on every input.
`add_one_disproved.cpp` does not return on `x == 42`, so SAW
correctly reports DISPROVED with that counterexample — the right
outcome, since silently treating a throwing function as equivalent
to a total spec is exactly the bug verification should catch.

If the intent is "never throws on valid inputs," the Cryptol spec
should encode the precondition or return `Option`/`Result`. SAW will
then re-prove under that contract.

> The pipeline does not need the
> [`saw-tools/exception-lower`](https://github.com/GaloisInc/saw-script/tree/master/saw-tools/exception-lower)
> pre-pass for MSVC — that tool targets Itanium EH (`__cxa_throw` /
> `invoke` / `landingpad`) and is still the right choice for Linux,
> macOS, or `*-pc-windows-gnu`.

## Running

```powershell
# Control case — VERIFIED
./verify.ps1 -CppFile tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_verified.cpp `
             -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one

# Throws case — DISPROVED (x = 42)
./verify.ps1 -CppFile tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_disproved.cpp `
             -CryptolSpec tests\e2e\cases\02-havoc-coverage\throws_exception\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one
```

Both are regression-tested via
[`cases.psd1`](../../../cases.psd1) /
[`Run-E2ETests.ps1`](../../../Run-E2ETests.ps1).
