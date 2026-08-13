# Proposal: Compositional contract overrides for cross-TU callees

**Status:** Implemented — **automatic**, no config required
**Audience:** saw-spec-gen + pretty-specs maintainers
**Author:** demo_protocol verification
**Date:** 2026-08-12

> **Implemented and automatic.** saw-spec-gen discovers cross-TU callees
> that appear only as a `declare` and, when the Cryptol spec defines a
> matching contract (by the existing `<name>` / `<name>_spec` naming
> convention, e.g. callee `double_it` ↔ `double_it_spec`), installs that
> contract via `llvm_unsafe_assume_spec` instead of the default
> fresh-return/havoc extern model. **No TOML is needed** — the linkage is
> inferred from the callgraph and the Cryptol spec.
>
> An optional explicit override remains for non-conventional names:
> `[functions.<caller>].compose = [{ cryptol_fn = "...", symbol = "..." }]`
> (plus `function = "..."` for `extern "C"` callees and
> `combine_scope = "callgraph"|"explicit"`). Self-composition (the simplest
> compose cycle) is rejected. E2E coverage:
> `tests/e2e/cases/13-compositional/double_plus_one/` — VERIFIED purely by
> automatic discovery (the caller proves *only* because the contract is
> composed), DISPROVED when the caller logic is wrong.

---

## 1. Problem

C++ is compiled per translation unit (TU). A function defined in one TU and
*called* from another appears in the caller's bitcode only as a body-less
`declare`. Today, when `verify-cpp`/`gen-verify` symbolically executes a
function `B` that calls a function `A` defined in a different TU, `A` has no
body in `B`'s module, so it is handled as an **unspecified extern**: the
generated `verify.saw` synthesizes an override that returns a **fresh symbolic
value** and havocs any memory reachable through `A`'s pointer arguments
(`extern_override_scan`).

That model is *sound but maximally imprecise*: it assumes `A` can return
anything. Any property of `B` that depends on `A`'s actual behaviour therefore
**cannot be proven**, even when `A` has already been verified on its own.

Two concrete symptoms observed in demo_protocol:

1. **False failures in batch/pipeline runs.** `pretty-specs --pipeline` feeds
   every model function to every `--impl` TU. A leaf like `provisionKey`
   (defined in `decision.cpp`) is *also* called from `controller.cpp`; against
   `controller.cpp`'s module it is `declare`-only, so the attempt returns
   `inconclusive`, which the manifest then counts as a proof failure — even
   though the same function `verified` against its defining TU. (See
   `pipeline.log`: 6 decision functions reported as failures purely because
   they are referenced from `controller.cpp`/`auth.cpp`.)

2. **No path to end-to-end proofs.** We have proven 13 leaf functions
   (6 pure decision fns + 5 KeyStore stateful methods + `canonicalize_lp` +
   `classifyCanonicalHost`). We *cannot* currently verify the orchestration
   functions that call them (`FleetController::handle_provision`,
   `handle_activate`, `getStatus` wiring) because their cross-TU callees are
   modeled as havoc.

## 2. Proposal

Add **compositional contract overrides** to `verify-cpp`/`gen-verify`:

> When verifying a function `B` that calls a function `A` for which a **proven
> Cryptol contract** exists, install that contract as an
> `llvm_unsafe_assume_spec` override for `A` — instead of the unspecified
> (fresh-return + havoc) extern override.

This is standard **assume-guarantee / compositional verification**:

- Prove `A ⊨ spec_A` once (already done for our leaves).
- When proving `B`, **assume** `spec_A` at every call to `A`.
- Soundness follows because each `A` is discharged separately; the assumption
  is never used to prove itself.

Concretely, two cooperating pieces:

### 2a. Callgraph-scoped module availability
To install an override for `A`, `A`'s symbol must be present. Provide the
defining TU(s) of the callees so the symbol resolves. Prefer
**callgraph-scoped `llvm_combine_modules`** (combine only the modules reachable
from the target `B`) over "combine everything", to avoid pulling unrelated
STL/exception machinery into leaf proofs.

### 2b. Auto-override callees with their proven contracts
For each callee `A` that has an entry in the proof manifest / config with a
`cryptol_fn` contract, emit:

```saw
A_contract <- llvm_unsafe_assume_spec m "<mangled A>" A_spec_from_cryptol;
```

and register it in `B`'s override list — *instead of* the fresh-return extern
override. Fall back to the unspecified override only when no contract exists
(preserving today's behaviour for genuinely-external symbols).

### 2c. Config surface (sketch)
```toml
[functions.handle_provision]
# Callees to resolve from their defining TUs and override with proven contracts.
compose = [
  { cryptol_fn = "provisionKey",  symbol = "?provisionKey@sdep@@..." },
  { cryptol_fn = "keyStoreProvisionRet", function = "provision" },
]
# Optional: cap module combination to the callgraph closure of the target.
combine_scope = "callgraph"   # or "explicit" with an impl list
```

`function`/`symbol` reuse the existing name-resolution the tool already needs
for the Cryptol-fn → C++-symbol mapping.

## 3. Soundness

Assume-guarantee with **non-circular** dependencies is sound:

- Each leaf `A` is proven against `spec_A` with no assumptions about its
  callers.
- `B`'s proof *assumes* `spec_A`. Since `A`'s proof does not depend on `B`,
  there is no circular reasoning.
- The tool should **reject cycles** in the compose graph (or require an
  explicit ranking/variant argument for recursion) to keep this sound.

## 4. Precision: contract override vs. symbolic execution of A

The reason to prefer overrides is precision *is preserved* when the contract is
complete, while cost and fragility drop.

| Contract shape | Precision vs. inlining `A`'s body | Notes |
|---|---|---|
| **Complete/exact** functional spec (full input→output relation, exact precond, exact frame) | **No loss** — observationally identical to inlining | This is what an equivalence proof (`C++ ≡ Cryptol` over all inputs) yields. All 13 demo_protocol contracts are of this kind. |
| **Partial** spec (result loosely constrained) | **Loss** — caller sees only the weaker fact | Only use when a full spec is intentionally out of scope. |
| **Over-approximate frame** (claims A writes memory it doesn't) | **Loss** on that memory | Havocs more than reality. |
| **Under-approximate frame** (claims A writes less than it does) | **UNSOUND** — not merely imprecise | Frame must be exact. |
| **Underspecified return** (fresh value under a predicate) | **Loss** | e.g. today's extern havoc override — the maximal case. |

Two effects that are **not** precision loss:

- **Preconditions.** A contract proven under precondition `P` adds an
  obligation that the caller establishes `P` at the call site. Raw symbolic
  execution might silently explore inputs outside `P` (potentially UB); the
  override surfaces the requirement instead. This is a *feature*.
- **Cost/fragility.** Symbolic execution re-runs `A`'s body at every call site
  and pulls in `A`'s own dependencies (STL, funclets), which can hit bitcode
  parser limits. The override avoids both.

**Bottom line:** for a complete functional contract there is **zero precision
loss**; you would only symbolically execute `A` when you *lack* such a contract
(or deliberately want a partial one).

## 5. Requested E2E test

Add a fixture demonstrating that `B` **only verifies when `A`'s contract is
used**, and **fails (is disproved) when `A` is modeled as havoc**. This directly
guards the feature.

### 5a. Source — two TUs

`a.cpp` (defines `double_it`):
```cpp
// TU A
#include <cstdint>
std::int32_t double_it(std::int32_t x) { return x * 2; }
```

`b.cpp` (defines `double_plus_one`, calls `double_it` across the TU boundary):
```cpp
// TU B
#include <cstdint>
std::int32_t double_it(std::int32_t x);            // declaration only in B
std::int32_t double_plus_one(std::int32_t x) {
    return double_it(x) + 1;                        // cross-TU call
}
```

### 5b. Cryptol specs
```cryptol
double_it_spec : [32] -> [32]
double_it_spec x = x * 2

double_plus_one_spec : [32] -> [32]
double_plus_one_spec x = x * 2 + 1
```

### 5c. Cases

**VERIFIED — with the compositional contract override**
```toml
[functions.double_plus_one_spec]
function = "double_plus_one"
compose  = [ { cryptol_fn = "double_it_spec", function = "double_it" } ]
combine_scope = "callgraph"
```
Expected: `double_plus_one` proves, because `double_it(x)` is overridden by
`double_it_spec x = x*2`, so the obligation is `x*2 + 1 == x*2 + 1`.

**DISPROVED — with `double_it` left as the unspecified (havoc) extern**
```toml
[functions.double_plus_one_spec]
function = "double_plus_one"
# no `compose`: double_it resolves to the fresh-return extern override
```
Expected: `double_plus_one` is **disproved**. Under havoc `double_it(x)`
returns a fresh `v`, so the obligation `v + 1 == x*2 + 1` fails (counterexample
`v ≠ x*2`, e.g. `v = 0, x = 5`).

The pair pins the exact behaviour: **B's correctness is recoverable only by
composing A's proven contract; the havoc model is provably insufficient.**

Suggested location, mirroring existing layout:
`tests/e2e/cases/13-compositional/double_plus_one/`
with `..._verified` (compose) and `..._disproved` (havoc) variants, following
the `09-stateful` / `08-overrides` fixture conventions.

## 6. Implementation notes (where this lands in saw-spec-gen)

- **`extern_override_scan`** — when a `declare`-only symbol has a manifest/config
  contract, emit an `llvm_unsafe_assume_spec` from that contract rather than the
  fresh-return unspecified override.
- **Module loading** — support callgraph-scoped `llvm_combine_modules` so callee
  bodies/symbols are present; keep leaf proofs single-module when they have no
  compose entries.
- **Name resolution** — reuse the Cryptol-fn → C++-symbol mapping the pipeline
  already needs (also fixes the separate `keyStoreProvisionRet ↔ provision`
  pipeline gap).
- **Pipeline aggregation (pretty-specs)** — independently, treat a
  `declare`-only symbol in a non-defining TU as *not present* (skip), and
  aggregate multiple per-TU results as **best-wins** (`verified` > `inconclusive`)
  so leaf functions stop being marked failed merely because a caller TU
  references them.
- **Cycle detection** — reject cyclic `compose` graphs (or require an explicit
  measure) to preserve non-circular assume-guarantee soundness.

## 7. Payoff for demo_protocol

- Removes the 6 false "proof failures" in the pipeline (Bug 1).
- Lets us verify the `FleetController` orchestration functions end-to-end using
  the 13 already-proven leaf contracts — turning a pile of independent leaf
  proofs into a real end-to-end guarantee.
