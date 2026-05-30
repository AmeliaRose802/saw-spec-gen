# STL coverage tests

This directory exercises `saw-spec-gen` against "typical C++" code
that pulls in the standard library — `<algorithm>`, `<numeric>`,
`<utility>`, `<tuple>`, `<memory>`, `<vector>`. Every test in this
suite compares a tiny `add_one(x: uint32_t) -> uint32_t` function
to the Cryptol spec `add_one_spec x = x + 1`. The "interesting"
work happens in how `add_one` reaches its return value: via
`std::max`, via `std::pair{x+1, x}.first`, via `std::vector::back`,
etc.

## Why every case currently RESOLVES TO DISPROVED

The tool's compositional havoc model classifies any function whose
declaration lives in a system include path (MSVC SDK, libstdc++,
LLVM headers) as an external symbol and emits an adversarial
override for it. The override is intentionally weak: it lets the
solver pick **any** return value, with no relationship to the
inputs. So a call like

```cpp
uint32_t y = x + 1;
return std::max(y, y);  // mathematically equal to x + 1
```

becomes "return value of `std::max` is unconstrained" in the SAW
proof goal, and the equivalence against `add_one_spec` is
trivially refuted with a counterexample.

This is documented behaviour, not a bug. The suite exists to:

1. **Document the gap** so contributors discover it before
   re-reporting the same issue.
2. **Catch regressions in the gap.** If a future tool change
   strengthens overrides for the math-only subset of `<algorithm>`,
   the `*_verified.cpp` cases will flip to `VERIFIED`. The runner
   will then report the case as failing-against-expected, and the
   author can update `Expected` to `VERIFIED` in `cases.psd1` and
   celebrate the improvement.
3. **Exercise the C++ frontend.** Even when the proof is refuted,
   the pipeline runs: clang → AST dump → filter → spec generation
   → SAW load → solver. Any crash, hang, or wrong-shape spec along
   the way gets caught by this suite even if the verdict itself is
   uninteresting.

## Coverage matrix

| Case                       | STL feature                                    | Expected | Notes |
|----------------------------|------------------------------------------------|----------|-------|
| `algorithm_max/`           | `std::max<uint32_t>(a, b)`                     | DISPROVED | Havoc gap on `<algorithm>`. |
| `algorithm_min/`           | `std::min<uint32_t>(a, b)`                     | DISPROVED | Havoc gap on `<algorithm>`. |
| `algorithm_clamp/`         | `std::clamp<uint32_t>(v, lo, hi)` (C++17)      | DISPROVED | Havoc gap on `<algorithm>`. Requires `CxxStandard = 'c++17'`. |
| `numeric_accumulate/`      | `std::accumulate(begin, end, init)`            | DISPROVED | Havoc gap on `<numeric>`; plus iterator havoc. |
| `pair_first/`              | `std::pair<u32, u32>{a, b}.first`              | DISPROVED | Pair constructor lives in `<utility>` → havoced. |
| `tuple_get/`               | `std::tuple<u32, u32>{a, b}` + `std::get<I>`   | DISPROVED | Both construction and `get` are `<tuple>` symbols. |
| `vector_back_havoc/`       | `std::vector::push_back` + `back()`            | DISPROVED | Compositional gap: allocator + buffer write/read can't be tied to the same value by the havoc model. |
| `unique_ptr_deref_havoc/`  | `std::make_unique<u32>(v)` + `*p`              | DISPROVED | Smart-pointer analog of vector gap; `operator new` + placement write are independent allocations under the override. |

Each directory ships:

* `add_one_verified.cpp` — code that *should* match `add_one_spec`
  but doesn't, due to the gap above.
* `add_one_disproved.cpp` — code that genuinely returns the wrong
  value (e.g. `x + 2`) and gets disproved for the *right* reason.
* `add_one_spec.cry` — the shared `add_one_spec x = x + 1` Cryptol
  spec.

## Running

```powershell
# Whole STL suite.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage

# Single case.
pwsh -NoLogo -NoProfile -File verify.ps1 `
    -CppFile tests/e2e/cases/10-stl-coverage/algorithm_max/add_one_verified.cpp `
    -CryptolSpec tests/e2e/cases/10-stl-coverage/algorithm_max/add_one_spec.cry `
    -CryptolFn add_one_spec -Function add_one

# C++17-required case via the runner.
pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage
```

The runner forwards `CxxStandard`, `ClangFlags`, and `IncludeDirs`
from the manifest to `verify.ps1`, so `algorithm_clamp/` gets
`-std=c++17` automatically.

## Two real bug fixes landed alongside this suite

While building the suite, two latent bugs in `saw-spec-gen` were
caught and fixed:

1. **System-header virtual methods leaking through the path
   filter** (`src/parsers/clang_ast/virtual_methods.rs`). When the
   user code did `#include <algorithm>`, the path filter would
   keep `namespace std { class exception { virtual const char *
   what(); … }; }` wrappers, and the virtual-method extractor
   would emit a havoc spec + vtable stub for `std::exception::what`.
   The auto-generated return-type lowering disagreed with the
   stub's pointer signature, so SAW aborted on `Incompatible types
   for return value when verifying exception_what_stub`. Fix:
   skip classes declared in any system header
   (`is_system_include_path` propagated through a parallel
   loc-tracking stack on the visitor). New regression test:
   `skips_virtual_methods_from_system_headers`.
2. **Uninitialised return pointee in havoc specs**
   (`src/emit/saw_emit/llvm_return.rs` and
   `src/emit/saw_emit/llvm_spec.rs`). The havoc model for a
   pointer-returning function emitted `ret_ptr <- llvm_alloc T;
   llvm_return ret_ptr;` with no `llvm_points_to`. Any caller
   that dereferenced the returned pointer hit "Error during
   memory load" because the cell was never initialised. Fix: pair
   the alloc with a fresh symbolic value and a `llvm_points_to`
   so the solver can observe the value it was supposed to pick
   freely.

Without these fixes, every test in this directory would have
aborted at SAW load time instead of resolving to a clean
DISPROVED verdict.

## When (not if) the gap closes

A future improvement might add a "smart" override for the
math-only subset of `<algorithm>` / `<utility>` (e.g. recognise
`std::max<T>` as `\\ x y -> if x >= y then x else y` and emit a
value-preserving spec instead of a pure-havoc one). When that
lands, the `*_verified.cpp` cases will start passing — flip their
`Expected` to `VERIFIED` in `cases.psd1` and update this README
in the same PR.
