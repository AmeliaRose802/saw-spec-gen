# Proposal: contract-oriented model for multi-effect functions

**Status:** Implemented (record-returning contract + field-projection bindings)
**Audience:** saw-spec-gen maintainers
**Companion:** `25-compositional-contract-overrides.md`
**Date:** 2026-08-12

> **Implemented.** A single record-returning Cryptol contract can supply
> every effect of a function via field projection. Config (per-function):
> `contract_return = "FIELD"` binds the return value to
> `(<cryptol_fn> args).FIELD`, and each `contract_ensures = ["REGION=FIELD"]`
> binds out-region `REGION`'s post-state to `(<cryptol_fn> args).FIELD`
> (region shape still declared via `out_buffer_param`). The model name is
> the `--cryptol-fn`, so it can match the C++ symbol — no `…Ret`/`…Post`
> split and no out-of-band name map. The existing `cryptol_fn_out` /
> split-function form still works (it is the two-clause special case).
> E2E: `tests/e2e/cases/14-contract/bump/` — VERIFIED (both clauses from
> one contract), DISPROVED (wrong `outPost` ⇒ the `ensures` clause is
> checked, not defaulted away).

---

## 1. Problem

A C++ function with more than one observable effect — a return value **and**
memory it mutates (`this`, out-buffers, globals) — is currently modeled by
**multiple separate Cryptol functions**, one per SAW assertion:

| C++ symbol | return spec | post-state spec |
|---|---|---|
| `KeyStore::provision` | `keyStoreProvisionRet` | `keyStoreProvisionPost` |
| `KeyStore::activate`  | `keyStoreActivateRet`  | `keyStoreActivatePost`  |
| `canonicalize_lp`     | `canonicalize_lp_ret`  | `canonicalize_lp_post`  |

This is because `gen-verify` sources **one Cryptol term per assertion**:
`--cryptol-fn` → `llvm_return`, `--cryptol-fn-out` → `llvm_points_to`. Two
assertions ⇒ two Cryptol functions.

Consequences:

- **Symbol proliferation.** One C++ function shows up as 2+ model functions,
  none of which alone *is* the function.
- **Name-mapping burden.** The model names can't equal the C++ symbol
  (`keyStoreProvisionRet` ≠ `provision`), so every downstream consumer
  (pipeline, compose overrides, coverage) needs an out-of-band mapping.
- **Frame is implicit.** "Everything else is unchanged" isn't stated as part of
  a contract; it lives in the hand-tuned post function.

## 2. The correct ontology

A function contract is a Hoare triple:

```
{ precondition }   f(args)   { postcondition }
```

The **postcondition is a conjunction of clauses over the post-state**:

- a **return** clause (`\result == …`),
- one **post-state** clause per mutated region (`*this' == …`, `*out' == …`),
- the **frame** (all other memory unchanged).

The return value is **one clause of the postcondition**, not a separate
category. Mutated `this` is a **world/heap postcondition**. Every mature spec
language models it this way — Dafny / ACSL / JML / F\* write **one** method
contract with `requires` + multiple `ensures`, one of which mentions `\result`.

Crucially, **SAW already has this shape.** A hand-written `llvm_verify` spec is
a single `do` block: preconditions, `llvm_execute_func`, then a `llvm_return`
**and** any number of `llvm_points_to` postconditions — all one contract. Only
the *auto-generation* layer splits it into per-assertion Cryptol functions.

## 3. Proposal

Introduce a **single contract abstraction per C++ function**: one named
Cryptol contract carrying every clause, which `gen-verify` binds to the right
SAW assertions.

### 3a. Cryptol shape — one record-returning function
```cryptol
// One contract for KeyStore::provision. Inputs = pre-state + args.
// Output = a record with named post-state and return clauses.
keyStoreProvision :
    [KS_BYTES][8] -> [64][8] ->
    { thisPost : [KS_BYTES][8]     // *this after the call  (heap postcondition)
    , ret      : [72][8]           // returned std::optional  (return clause)
    }
```

The model name now **matches the C++ symbol** (`keyStoreProvision` ↔
`provision`); the two effects are *fields*, not separate symbols.

### 3b. Config — bind clauses to assertions
```toml
[functions.keyStoreProvision]
function = "provision"
# Precondition clauses (requires):
preconditions = ["(this_pre @ 128) <= 1", "(this_pre @ 144) <= 1"]
# Postcondition clause bindings (ensures):
return       = ".ret"                    # -> llvm_return
[[functions.keyStoreProvision.ensures]]
region  = "this"                         # the object out-buffer (152 bytes)
field   = ".thisPost"                    # -> llvm_points_to this_ptr
# Frame: regions not listed are asserted unchanged by default.
```

`in_buffer_size` / `out_buffer_param` stay as the region-shaping mechanism; the
new part is that **all clauses come from one contract function** via field
projection, instead of from N top-level functions.

### 3c. Back-compat (desugaring)
Keep today's `--cryptol-fn` / `--cryptol-fn-out` (and their toml equivalents)
as **sugar** that desugars to a two-clause contract. Existing specs keep
working; the split-function form becomes one special case of the contract form.

## 4. Benefits

- **One symbol per function.** The model function *is* the function; no
  `…Ret`/`…Post` shrapnel.
- **Names match.** `keyStoreProvision` ↔ `provision`, so the pipeline,
  coverage, and compose overrides resolve **without an out-of-band map** —
  this directly removes the name-mapping burden from
  `26-pipeline-multi-tu-coverage-and-compositional-rendering.md`.
- **Frame is explicit and default-safe.** Unlisted regions are asserted
  unchanged, matching how Dafny/ACSL treat frames.
- **Composition is cleaner.** `25-compositional-contract-overrides.md` assumes
  a callee's contract as an override. A single contract *is* exactly the object
  to assume — return + heap effects together — instead of stitching a `…Ret`
  and a `…Post` back into one override.
- **Reads like a spec.** `requires` / `ensures return` / `ensures *this'` is the
  standard vocabulary; reviewers see one contract, not a scattered pair.

## 5. Precision / soundness

No change to the underlying logic — it is the *same* set of SAW assertions,
sourced from one function instead of several. A record-returning Cryptol
function is total and pure; projecting `.ret` / `.thisPost` is exact. The
default-frame rule ("unlisted regions unchanged") must be **checked**, not
assumed, so it stays sound (an omitted mutation becomes a proof failure, not a
silent pass).

## 6. Requested E2E

Extend the stateful fixtures (`09-stateful/*`) with a **single-contract**
variant of an existing mutating method:

- Define the contract as one record-returning Cryptol function
  (`{ thisPost, ret }`).
- Config binds `return = ".ret"` and an `ensures` clause for `this = ".thisPost"`.
- Assert it **verifies** and is **byte-identical** to the current
  split-function result (`…Ret` + `…Post`) — proving the contract form is a
  faithful, lossless replacement.
- Add a **disproved** twin where the `ensures this` clause is wrong, to confirm
  the heap postcondition is actually checked (not defaulted away).

## 7. Relationship to the other proposals

- `25` (compositional overrides) — *what* to assume for a cross-TU callee.
- `26` (pipeline coverage/rendering) — *how* results are aggregated and shown.
- `27` (this) — *how a single multi-effect function is modeled at all*.
  Landing `27` shrinks the mapping problem `26` has to solve and gives `25` a
  single clean object to assume per callee.
