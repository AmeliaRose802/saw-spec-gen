# One-pager: why `activate` still fails after #57

**Audience:** saw-spec-gen team  
**Context date:** 2026-07-09  
**Target:** `KeyStore::activate` in `demo_protocol/cpp/src/key_store.cpp`

## Executive summary

The recent #57 fix is present and working for its intended regression test, but
`activate` still fails in this repro because generation emits an `Unknown`
vtable artifact that is internally inconsistent:

- `verify.saw` requires loading `vtable_stubs.bc`
- generation only emits `vtable_stubs.ll`
- that `.ll` cannot assemble because it references undefined symbols:
  `@unknown__destroy_stub` and `@unknown__delete_this_stub`

So verification fails before we reach the actual function proof obligations.

## Are we using the locally compiled fixed binary?

Yes.

Evidence from local machine:

- branch: `copilot/emit-struct-typed-allocations`
- HEAD: `9e2ef6347855b90849984c3f9092fa4b76813567`
- commit subject at HEAD: `Gate vtable-stub emission on virtual-dispatch reachability (#57)`
- binary path used: `C:\Users\ameliapayne\saw-spec-gen\target\release\saw-spec-gen.exe`
- binary version: `saw-spec-gen 0.1.2`

So this is not a stale binary issue.

## Repro

Run native end-to-end path (the same path that does auto exception-lowering):

1. `saw-spec-gen verify-cpp --cpp-file <...>key_store.cpp --cryptol-spec <...>SDEP_cpp.cry --cryptol-fn keyStoreActivateRet --function activate --output <...>out_verifycpp --include-dir <...>cpp/include --cxx-standard c++20 --extra-spec-gen-arg=--out-buffer-param --extra-spec-gen-arg=this=152 --extra-spec-gen-arg=--cryptol-fn-out --extra-spec-gen-arg=this=keyStoreActivatePost --extra-spec-gen-arg=--in-buffer-size --extra-spec-gen-arg=keyId=16`
2. SAW load fails because `verify.saw` attempts to load `vtable_stubs.bc`, which is absent.

Observed generated artifacts in output dir:

- `verify.saw` includes:
  - `m_stubs <- llvm_load_module "vtable_stubs.bc"`
  - warning comment says `.bc` was not auto-assembled
- `interface_overrides.saw` includes only `alloc_unknown_this` helper
  and `all_interface_overrides = []`
- `vtable_stubs.ll` contains only:
  - `@unknown_vtable = global [2 x ptr] [ ptr @unknown__destroy_stub, ptr @unknown__delete_this_stub ]`
  - no definitions for those stub symbols

Manual assembly confirms invalid IR:

- `clang -c -emit-llvm vtable_stubs.ll -o vtable_stubs.bc`
- error: `use of undefined value '@unknown__delete_this_stub'`

## Why this still happens even after #57

#57 correctly gates many incidental-polymorphism cases (its e2e regression case
passes), but this repro appears to take a different path:

- some `Unknown` class/vtable setup is still emitted,
- there are no corresponding reachable virtual methods,
- therefore no method stubs are emitted,
- but the `Unknown` vtable global is still emitted and references missing stubs.

This leaves generation in a contradictory state: "no overrides needed" but
"load stubs module anyway".

## Likely root-cause class

A class-constructor/vtable helper emission path can still fire for `Unknown`
without a matching non-empty method set after reachability filtering.
The current guard appears to be method-reachability-centric, while an
independent constructor/vtable path still emits a shell vtable.

## What should be true

For any target where reachable virtual method set is empty:

1. do not emit `interface_overrides.saw` vtable helpers,
2. do not emit `vtable_stubs.ll`,
3. do not reference `vtable_stubs.bc` in `verify.saw`,
4. use single-module load path only.

Additionally, generation must never emit a vtable global referencing undefined
stub symbols.

## Suggested fixes

1. **Hard invariant in emitter:** before writing any vtable global, assert every
   referenced stub symbol is also emitted in the same module.
2. **Single source of truth for "has interface stubs":** drive
   `verify.saw` module-loading decision from emitted stub count, not from class
   helper presence.
3. **Filter `Unknown` shell classes:** if no reachable methods survive for a
   class, drop the class entirely from vtable emission.
4. **Regression test:** add a Windows/MSVC `activate`-style fixture where
   exception-lowering + mutex + no virtual dispatch coexist, and assert no
   stubs are emitted and script is runnable as generated.

## Scope note

Exception-lowering is not the current blocker in this repro; it is running via
`verify-cpp` path. The blocker here is invalid/spurious vtable-stub generation.
