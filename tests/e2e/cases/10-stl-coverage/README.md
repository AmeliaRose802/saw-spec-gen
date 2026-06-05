# STL coverage tests

This directory exercises `saw-spec-gen` against "typical C++" code
that pulls in the standard library — `<algorithm>`, `<numeric>`,
`<utility>`, `<tuple>`, `<memory>`, `<vector>`. Every test in this
suite compares a tiny `add_one(x: uint32_t) -> uint32_t` function
to the Cryptol spec `add_one_spec x = x + 1`. The "interesting"
work happens in how `add_one` reaches its return value: via
`std::max`, via `std::pair{x+1, x}.first`, via `std::vector::back`,
etc.

> The default e2e suite (`pwsh tests/e2e/Run-E2ETests.ps1`)
> intentionally omits the `stl_coverage` tag — these cases are
> research carve-outs that require the libstdc++/MSVC STL headers
> already on the build host. Run them with `-Tag stl_coverage`
> (or `-All`) once you have SAW + a C++ toolchain wired up.

## Two complementary fixes — `saw_spec_gen-9x5` + `-jpp`

The cases in this directory used to all `RESOLVE TO DISPROVED`
for algebraically correct code. Two changes (landed together)
fixed that:

1. **`saw_spec_gen-9x5` — Crucible-safety analyzer**
   (`src/transform/crucible_safety.rs`). Replaces the blanket
   "any function in a system header is a havoced extern" rule
   with a two-step gate:
   - Does the callee have a body in the bitcode?
   - Does the body use only opcodes / intrinsics Crucible can
     lower without an external override? (See the allow-list in
     `crucible_safety.rs`.)
   If both answer yes, the call-graph walker descends into the
   body and SAW symbolically executes it. This is what flips
   `std::max`, `std::min`, `std::clamp`, `std::pair::first`,
   `std::tuple::get`, and `std::accumulate` over a concrete
   `std::array` from `DISPROVED` to `VERIFIED`.

2. **`saw_spec_gen-jpp` — STL functional-override registry**
   (`src/emit/saw_emit/stl_overrides.rs`). A hard-coded
   pattern list that forces specific STL family symbols
   (`std::vector`, `std::unique_ptr`, `std::shared_ptr`,
   `std::basic_string`, `std::list`/`deque`,
   `std::map`/`set`/`unordered_*`) to be treated as externals
   even when the safety analyzer would let them through.
   This stops Crucible from descending into libstdc++ allocator
   helpers that crash it with `Cannot mux LLVM values`
   (`v1: <int8>` muxed against `v2: <int1>` in the SSO branch
   of `_M_realloc_insert`). The adversarial-spec emitter then
   havocs the return value, which is enough to recover a clean
   `DISPROVED` verdict for the existing havoc test cases.

The two passes are independent and individually killable:

```bash
# Revert to the pre-9x5 "everything system is external" rule.
SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1 pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage

# Disable the jpp registry so libstdc++ container helpers go back to
# panicking inside Crucible (UNKNOWN).
SAW_SPEC_GEN_NO_STL_OVERRIDES=1 pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage

# Both off at once = baseline behaviour before this branch.
SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1 SAW_SPEC_GEN_NO_STL_OVERRIDES=1 \
    pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage
```

## Coverage matrix

| Case                       | STL feature                                    | Expected      | Notes |
|----------------------------|------------------------------------------------|---------------|-------|
| `algorithm_max/`           | `std::max<uint32_t>(a, b)`                     | **VERIFIED**  | Body is `alloca`/`load`/`store`/`icmp`/`br` — `9x5` safety check accepts it. |
| `algorithm_min/`           | `std::min<uint32_t>(a, b)`                     | **VERIFIED**  | Same as above. |
| `algorithm_clamp/`         | `std::clamp<uint32_t>(v, lo, hi)` (C++17)      | **VERIFIED**  | Requires `CxxStandard = 'c++17'`. |
| `numeric_accumulate/`      | `std::accumulate(begin, end, init)`            | **VERIFIED**  | Iterators over a concrete `std::array` resolve to pointer arithmetic; loop body is safe. |
| `pair_first/`              | `std::pair<u32, u32>{a, b}.first`              | **VERIFIED**  | Pair constructor body is plain `getelementptr` + `store`/`load`. |
| `tuple_get/`               | `std::tuple<u32, u32>{a, b}` + `std::get<I>`   | **VERIFIED**  | Same as `pair_first`, with variadic-template scaffolding. |
| `vector_back_havoc/`       | `std::vector::push_back` + `back()`            | **DISPROVED** | `jpp` forces `vector::{ctor,push_back,back,dtor}` to havoc — `back()` returns a fresh symbolic `u32`, so the equivalence is unprovable. The original symptom was an UNKNOWN/Crucible panic. |
| `unique_ptr_deref_havoc/`  | `std::make_unique<u32>(v)` + `*p`              | **DISPROVED** | Same shape as above, via `unique_ptr` family patterns. |

The `*_havoc/` cases are intentionally retained as DISPROVED
witnesses: they verify that the `jpp` registry is doing its
job (no Crucible panic) without claiming we can functionally
reason about `std::vector` element identity. That latter
work — proving `push_back(v); back() == v` — would require
the full Cryptol postcondition story from `jpp`'s scope
("Future work" below).

Each directory ships:

* `add_one_verified.cpp` (or, for the havoc cases,
  `add_one_gap_disproved.cpp`) — code that *should* match
  `add_one_spec`. The `_gap_disproved` suffix flags that the
  code is morally correct but the verifier can't yet *prove*
  the equivalence.
* `add_one_disproved.cpp` — code that genuinely returns the
  wrong value (e.g. `x + 2`) and gets disproved for the
  *right* reason.
* `add_one_spec.cry` — the shared `add_one_spec x = x + 1`
  Cryptol spec.

## Running

```powershell
# Whole STL suite (not in the default tag set).
pwsh tests/e2e/Run-E2ETests.ps1 -Tag stl_coverage

# Single case.
pwsh -NoLogo -NoProfile -File verify.ps1 `
    -CppFile tests/e2e/cases/10-stl-coverage/algorithm_max/add_one_verified.cpp `
    -CryptolSpec tests/e2e/cases/10-stl-coverage/algorithm_max/add_one_spec.cry `
    -CryptolFn add_one_spec -Function add_one
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
aborted at SAW load time instead of producing a clean verdict.

## Future work

The `jpp` registry is currently a "force-havoc" allow-list —
it stops Crucible from panicking but doesn't model what the
overridden methods actually do. To recover `VERIFIED` for
`vector_back_havoc/add_one_gap_disproved.cpp` we'd need
functional overrides in SAW + Cryptol that capture the
operational semantics (`push_back(v); back() == v`,
`*make_unique<T>(v) == v`, etc.). The scaffolding for that
work — the registry + override sites — is already in place;
the missing piece is writing the per-template Cryptol
postconditions. Tracked by the open bullet list in
`saw_spec_gen-jpp`.
