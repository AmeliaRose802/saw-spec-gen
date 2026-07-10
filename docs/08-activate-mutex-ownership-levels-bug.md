# One-pager: `activate` now fails in mutex ownership-level modeling (post-vtable fix)

**Audience:** saw-spec-gen team  
**Date:** 2026-07-09  
**Target function:** `KeyStore::activate` in `cpp/src/key_store.cpp`

## Executive summary

The recent vtable issue is fixed in the local build (no `vtable_stubs.*` emitted,
`verify.saw` uses single-module load path), but `activate` still fails during
symbolic execution with:

- `internal: error: in ?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ`
- `Error during memory load`

This is now the first blocker after vtable cleanup.

## What changed (and what did not)

### Fixed

- No spurious vtable combine path in generated script.
- Output contains only `verify.saw` + main artifacts (no `vtable_stubs.ll/.bc`).

### Still failing

- Proof fails in an internal call chain through MSVC mutex ownership checks,
  not at vtable loading.

## Repro (current local build)

Use native verify flow with object post-state model:

```powershell
saw-spec-gen.exe verify-cpp \
  --cpp-file C:\Users\ameliapayne\demo_protocol\cpp\src\key_store.cpp \
  --cryptol-spec C:\Users\ameliapayne\demo_protocol\cpp\saw\SDEP_cpp.cry \
  --cryptol-fn keyStoreActivateRet \
  --function activate \
  --output C:\Users\ameliapayne\demo_protocol\cpp\saw\_ks_probe\out_verifycpp \
  --include-dir C:\Users\ameliapayne\demo_protocol\cpp\include \
  --cxx-standard c++20 \
  --extra-spec-gen-arg=--out-buffer-param \
  --extra-spec-gen-arg=this=152 \
  --extra-spec-gen-arg=--cryptol-fn-out \
  --extra-spec-gen-arg=this=keyStoreActivatePost \
  --extra-spec-gen-arg=--in-buffer-size \
  --extra-spec-gen-arg=keyId=16
```

Observed SAW failure:

- enters proof, applies `_Mtx_lock`/`_Mtx_unlock`/`memcmp` overrides,
- then fails at `_Verify_ownership_levels` with `Error during memory load`.

`result.json` verdict currently records `DISPROVED` for this run.

## Key technical evidence from generated IR

From `out_verifycpp/key_store.ll`:

- `activate` calls lock path that reaches `_Verify_ownership_levels`.
- `_Verify_ownership_levels` is **defined** in-module as linkonce ODR
  (not a declare-only symbol):
  - `define linkonce_odr ... "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"`
- lock/unlock are declare-only and do receive assumed specs:
  - `_Mtx_lock` declared
  - `_Mtx_unlock` declared

Implication: pinning `_Mtx_lock/_Mtx_unlock` to success sentinel is not enough,
because simulator still steps into `_Verify_ownership_levels` (a defined helper)
and attempts typed reads over unconstrained mutex internals.

## Why this is likely happening

`this=152` models the entire `KeyStore` object as a flat byte array for the
state transition model. That is fine for the Cryptol `keyStoreActivatePost` byte
image, but `_Verify_ownership_levels` performs typed accesses over `_Mutex_base`
layout internals (ownership counters / levels) that are not explicitly
well-formed in the generated setup. SAW then hits a typed memory load failure.

In short: this is the same class of problem as earlier `scoped_lock` helper
issues, now localized to a different defined mutex helper.

## Suggested fixes in saw-spec-gen

1. **MSVC mutex helper abstraction pass (preferred):**
   treat known `_Mutex_base` verification helpers as abstract no-op/assume-spec
   boundaries in generated proofs when the user intent is sequential behavior.

2. **Defined-helper suppression policy:**
   allow safe abstraction of specific **defined** runtime helper symbols (not
   just declare-only externs), controlled by a curated allowlist for standard
   library lock-check internals.

3. **Auto `-O1` fallback for this signature:**
   if `activate` at `-O0` fails with this exact `_Verify_ownership_levels`
   memory-load signature, recompile/regenerate at `-O1` and retry automatically
   (the native verify flow already has a one-shot fallback mechanism in other
   scenarios).

4. **Regression fixture:**
   add a Windows/MSVC e2e fixture where a lock-guarded non-virtual method with
   in-module `_Verify_ownership_levels` verifies without manual script patching.

## Acceptance criteria for closure

- Generated verify flow reaches `llvm_verify` obligations without internal
  memory-load failure in `_Verify_ownership_levels`.
- No manual edits to generated `.saw` are required.
- Existing mutex-sentinel behavior remains intact (`_Mtx_lock/_Mtx_unlock`
  success pin).

## Scope note

This issue is independent of the prior vtable-stub bug. That one appears fixed
in this local build; this one is the next blocker surfaced afterward.
