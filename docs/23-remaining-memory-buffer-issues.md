# Remaining Memory Buffer Issues (2026-07-16)

This note isolates the still-open memory buffer failures in `verify-cpp` for KeyStore methods after recent saw-spec-gen fixes.

## Scope

In scope:

- `activate` (`keyStoreActivateRet`)
- `provision` (`keyStoreProvisionRet`)

Out of scope:

- `hasKey` / `isActive` (now `VERIFIED` on `fix/aligned-object-buffer`)
- non-buffer blockers (`canonicalizePayload` string override typing, `isValidSignature` hmac sub-callee arg mismatch)

## Repro Environment

- repo: `C:/Users/ameliapayne/demo_protocol`
- saw-spec-gen: `C:/Users/ameliapayne/saw-spec-gen`
- branch tested: `fix/aligned-object-buffer`
- binary: `target/release/saw-spec-gen.exe`

Common verify flags:

- `--cryptol-spec cpp/saw/SDEP_cpp.cry`
- `--include-dir cpp/include`
- `--cxx-standard c++20`
- `--clang-flag=-fexceptions`
- `--clang-flag=-fno-inline`

Artifacts:

- `cpp/saw/_recheck_2026_07_16_fixbranch/activate.log`
- `cpp/saw/_recheck_2026_07_16_fixbranch/provision.log`

## Current Result

| Function | Verdict | Primary signature |
| --- | --- | --- |
| `activate` | `DISPROVED` | `internal: error: in ??$equal@PEBEPEBE@std@@...` + `Error during memory load` |
| `provision` | `DISPROVED` | `internal: error: in ?provision@KeyStore...` + `Error during memory load` |

These are vacuous `DISPROVED` outcomes (internal simulator memory-load failure), not semantic mismatches between C++ and Cryptol.

## Update (2026-07-16, branch `fix/aligned-object-buffer`)

The vacuous whole-proof crash is **gone** — both methods now reach genuine
proof obligations:

- `activate` → `DISPROVED`, counterexample `rv = 0`. The unbroken path runs
  the STL `Uuid` comparison down to a `memcmp` extern. The extern override
  previously **havoc'd** the return (`rv <- llvm_fresh_var`), so the solver
  freely picked `rv == 0` ("Uuids equal") even for unequal ids — a false
  counterexample.
- `provision` → `DISPROVED`, one subgoal hits `Error during memory load`
  while checking the `std::optional<EnrollmentKey>` sret post-state. This is
  a sret-buffer post-state load, distinct from the `memcmp` issue.

### What was fixed here: faithful fixed-length `memcmp`

Implemented a **faithful `memcmp` override** (branch work in
`src/transform/memcmp_scan.rs`, `src/emit/saw_emit/memcmp_override.rs`): when
every `memcmp` call site in the module passes the **same compile-time-constant
length** `N`, the override models the exact equality decision — it returns `0`
iff the two `N`-byte buffers are equal — instead of havocing `rv`. This
eliminates the false-DISPROVED (and the symmetric false-VERIFIED) for the
common `memcmp(a, b, N) == 0` pattern. When the length is non-constant, it
falls back to a correctly pointer-typed havoc. `memcmp` is now owned entirely
by the bitcode extern-override scan; the old AST external-call auto-spec (which
modeled the `const void*` args as `llvm_int 64` and failed SAW structural
matching) is suppressed. Covered by e2e
`tests/e2e/cases/08-overrides/memcmp_fixed/` (VERIFIED + DISPROVED).

### Why `activate` is still not VERIFIED

The demo's `Uuid::operator!=` lowers (under `-fno-inline`, MSVC STL) to
`std::equal<const unsigned char*>`, which computes the compare length as a
**pointer difference** (`last1 - first1`) and passes it to `memcmp` as a
*register* value (`i64 noundef %25`), not a literal. So the constant-length
extractor correctly returns `None` and `memcmp` stays on the havoc fallback —
the faithful override does not apply. Making `activate` verify requires a
higher-level functional model of the `std::equal<PEBEPEBE>` range-compare (the
length is not statically recoverable at the `memcmp` call site), plus resolving
a further `No implementation or override found for pointer` on the
`equal_to<void>` predicate call. Filed as a follow-up.

`provision`'s remaining `Error during memory load` is the sret post-state load
of `std::optional<EnrollmentKey>` and is tracked separately (its DISPROVED is
also partly a Cryptol-model `isActive`-reset concern, not a tool bug).

## Why This Is Still a Buffer-Modeling Gap


Recent fixes improved object alignment handling enough for some paths (`hasKey`, `isActive`) to verify, but `activate` and `provision` still traverse nested STL/template paths that perform typed loads not safely supported by the current object-buffer model in generated scripts.

The failure moved from earlier `optional::has_value` sites to deeper internals (`std::equal` and provision-body path), which indicates partial progress but incomplete coverage of typed nested loads.

## Minimal Upstream Goal

`verify-cpp` should be able to symbolically execute these two methods without `Error during memory load` and reach ordinary proof obligations.

Acceptance criteria:

1. No `Error during memory load` for `activate` and `provision` under the repro configuration above.
2. Final verdict is proof-driven (`VERIFIED` or real `DISPROVED` with model-level counterexample), not simulator-internal failure.
3. Add/extend e2e coverage for this KeyStore shape so regressions are caught automatically.

## Suggested Fix Direction

Prioritize targeted handling for the nested STL/internal access patterns used by these two call paths, rather than broad script rewrites. The existing mitigation style (specific helper overrides and bounded object modeling adjustments) has already shown partial success and is the lowest-risk path to full unblock.

## Quick Repro Commands

```powershell
Set-Location 'C:\Users\ameliapayne\demo_protocol'
$ssg='C:\Users\ameliapayne\saw-spec-gen\target\release\saw-spec-gen.exe'

& $ssg verify-cpp --cpp-file 'cpp\src\key_store.cpp' --function 'activate' --cryptol-fn 'keyStoreActivateRet' --cryptol-spec 'cpp\saw\SDEP_cpp.cry' --include-dir 'cpp\include' --cxx-standard 'c++20' --clang-flag=-fexceptions --clang-flag=-fno-inline --output 'cpp\saw\_recheck_2026_07_16_fixbranch\out_activate'

& $ssg verify-cpp --cpp-file 'cpp\src\key_store.cpp' --function 'provision' --cryptol-fn 'keyStoreProvisionRet' --cryptol-spec 'cpp\saw\SDEP_cpp.cry' --include-dir 'cpp\include' --cxx-standard 'c++20' --clang-flag=-fexceptions --clang-flag=-fno-inline --output 'cpp\saw\_recheck_2026_07_16_fixbranch\out_provision'
```
