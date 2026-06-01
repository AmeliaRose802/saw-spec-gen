# saw-spec-gen bug: `sret` aggregate returns not detected for non-trivial types

**Date:** 2026-05-30
**saw-spec-gen commit:** `2071230` (branch `feat-enum-type-constraints`)
**Severity:** P2 — auto-generated `verify.saw` is non-compilable for any C++ function returning a non-trivial aggregate by value
**Reproducer in this repo:**
- C++ source: [cpp/include/sdep/decision.hpp](cpp/include/sdep/decision.hpp#L52-L65) (`getStatus`)
- LLVM IR (after build): [cpp/saw/out_getStatus/verify_targets.ll](cpp/saw/out_getStatus/verify_targets.ll)
- Auto-generated spec: [cpp/saw/out_getStatus/verify.saw](cpp/saw/out_getStatus/verify.saw)
- SAW log: [cpp/saw/out_getStatus/saw_run.log](cpp/saw/out_getStatus/saw_run.log)

---

## Summary

For any C++ function whose return type is an aggregate that the MSVC x64
ABI passes via a hidden `sret` pointer, `saw-spec-gen gen-verify` emits
an arg list that **omits the sret pointer**. SAW then refuses to verify
because the bitcode signature has `ptr sret(...)` in argument position
0 but the generated spec puts the first source-level scalar argument
there.

The existing sret support added in commit `a57513b`
(`saw_emit/gen_verify: sret aggregate returns, container-class this,
ctor filtering`) fires for some cases but not when the returned
struct contains a non-trivially-copyable / non-trivially-destructible
member (here: `std::optional<Uuid>`).

---

## Repro

### C++ under test

```cpp
// cpp/include/sdep/decision.hpp
struct EnrollmentStatus {
    FleetMode             fleetMode;
    bool                  hasKey;
    std::optional<Uuid>   keyId;   // non-trivial destructor → MSVC sret
    bool                  isActive;
};

[[nodiscard]] constexpr EnrollmentStatus
getStatus(bool fleetEnabled,
          bool hasKey,
          bool keyIsActive,
          const Uuid& keyId) noexcept;
```

### Compile + run

```powershell
cd cpp ; pwsh .\build.ps1 -NoRun       # produces verify_targets.bc/.ll
cd saw\out_getStatus
saw verify.saw
```

### LLVM IR (post-Clang)

```llvm
define linkonce_odr dso_local void
  @"?getStatus@sdep@@YA?AUEnrollmentStatus@1@_N00AEBUUuid@1@@Z"
  (ptr dead_on_unwind noalias writable
        sret(%"struct.sdep::EnrollmentStatus") align 1 %0,    ; <-- hidden sret
   i1 noundef zeroext %1,        ; fleetEnabled
   i1 noundef zeroext %2,        ; hasKey
   i1 noundef zeroext %3,        ; keyIsActive
   ptr noundef nonnull align 1 dereferenceable(16) %4)        ; const Uuid&
```

Note: the symbol mangling `?AUEnrollmentStatus@1@` in the return-type
slot of the MSVC mangled name **always** corresponds to sret on x64 —
this is determinable from the mangled name alone, without inspecting
the IR.

### Auto-generated `verify.saw` (wrong)

```saw
let getStatus_equiv_spec = do {
    fleetEnabled <- llvm_fresh_var "fleetEnabled" (llvm_int 1);
    hasKey       <- llvm_fresh_var "hasKey"       (llvm_int 1);
    keyIsActive  <- llvm_fresh_var "keyIsActive"  (llvm_int 1);
    keyId_ptr    <- llvm_alloc_readonly (llvm_array 16 (llvm_int 8));
    keyId        <- llvm_fresh_var "keyId" (llvm_array 16 (llvm_int 8));
    llvm_points_to keyId_ptr (llvm_term keyId);

    // BUG: missing sret pointer here in arg position 0
    llvm_execute_func [llvm_term fleetEnabled,
                       llvm_term hasKey,
                       llvm_term keyIsActive,
                       keyId_ptr];

    llvm_return (llvm_term {{
        getStatus_cpp (fleetEnabled ! 0) (hasKey ! 0) (keyIsActive ! 0) keyId
    }});
};
```

### Expected `verify.saw` (one possible shape)

```saw
let getStatus_equiv_spec = do {
    out_ptr      <- llvm_alloc (llvm_struct "struct.sdep::EnrollmentStatus");
    fleetEnabled <- llvm_fresh_var "fleetEnabled" (llvm_int 1);
    hasKey       <- llvm_fresh_var "hasKey"       (llvm_int 1);
    keyIsActive  <- llvm_fresh_var "keyIsActive"  (llvm_int 1);
    keyId_ptr    <- llvm_alloc_readonly (llvm_array 16 (llvm_int 8));
    keyId        <- llvm_fresh_var "keyId" (llvm_array 16 (llvm_int 8));
    llvm_points_to keyId_ptr (llvm_term keyId);

    llvm_execute_func [out_ptr,                       // <-- sret arg
                       llvm_term fleetEnabled,
                       llvm_term hasKey,
                       llvm_term keyIsActive,
                       keyId_ptr];

    // Postcondition over the buffer the function wrote, NOT llvm_return
    llvm_points_to out_ptr (llvm_term {{
        getStatus_cpp (fleetEnabled ! 0) (hasKey ! 0) (keyIsActive ! 0) keyId
    }});
};
```

(Plus the Cryptol shim has to return a sequence/struct whose layout
matches `EnrollmentStatus` — that's the user's problem, not the tool's.
Today this repo already returns `[20][8]` from `getStatus_cpp` which
is the right shape; only the SAW arg-binding is wrong.)

### SAW error

```
Stack trace:
   (builtin) in llvm_verify
   .\verify.saw:29:1-31:34 (at top level)
Type mismatch in argument 0 when verifying
  "?getStatus@sdep@@YA?AUEnrollmentStatus@1@_N00AEBUUuid@1@@Z"
Argument is declared with type: ptr
but provided argument has incompatible type: i1
Note: this may be because the signature of your function changed during
compilation. If using Clang, check the signature in the disassembled .ll file.
```

---

## Expected vs actual

| Aspect | Expected | Actual |
|---|---|---|
| Hidden sret arg | Inserted as LLVM arg 0 | Omitted |
| Result delivery | `llvm_points_to out_ptr ...` | `llvm_return ...` (no-op for `void` function) |
| LLVM signature seen | `void @f(ptr sret(T), ...)` | (same, but ignored by emitter) |

---

## Where in the code

Best guess (please confirm):
- `src/parsers/clang_ast/functions.rs` — sret detection from the C++ AST
  is most likely keying on a "trivially copyable" check, which `EnrollmentStatus`
  fails because of `std::optional<Uuid>`. But the actual ABI rule on MSVC x64
  is broader: any aggregate ≥ 9 bytes, or with a non-trivial copy ctor,
  or with a non-POD member, returns via sret.
- `src/emit/saw_emit/havoc.rs` / `verify_script_steps.rs` — confirm
  whether the sret path is selected for this function (the commit
  `a57513b` log suggests yes for some shapes — please dump which).

A robust alternative: **parse the LLVM IR directly** rather than
re-deriving the ABI from the AST. The bitcode generated by Clang
already has `sret(...)` on parameter 0 when applicable; using that as
ground truth would side-step ABI-rule maintenance entirely. This repo
already has both `verify_targets.bc` and `verify_targets.ll` adjacent
to the generated spec, so the IR is available at emit time.

---

## Suggested test case for saw-spec-gen's e2e suite

`tests/e2e/cases/<new>/sret_with_nontrivial_member.cpp`:

```cpp
#include <optional>
#include <cstdint>

struct Inner { std::uint64_t lo; std::uint64_t hi; };

struct Outer {
    int                 flag;
    std::optional<Inner> opt;   // non-trivial destructor
    int                 tail;
};

[[nodiscard]] Outer make_outer(int flag, bool has_inner, std::uint64_t v) {
    return Outer{
        .flag = flag,
        .opt  = has_inner ? std::optional<Inner>{{v, v}} : std::nullopt,
        .tail = ~flag,
    };
}
```

Assertion: the generated `verify.saw` must contain
`<x>_ptr <- llvm_alloc (llvm_struct "struct.Outer");` and pass it as
arg 0 to `llvm_execute_func`, and must not emit `llvm_return`.

---

## Workaround in the affected repo

None applied. We chose to leave the auto-spec broken in-tree as a clean
reproducer rather than hand-edit `verify.saw`. The property-level proofs
about `getStatus_cpp` go through at the Cryptol layer (Layer 3 of
`verify_all.ps1`, 29/29 PASS), so the only thing this bug blocks is the
"C++ matches Cryptol shim" link in the chain for this one function.

Once fixed, the SAW Cryptol shim `getStatus_cpp` in
[cpp/saw/SDEP_cpp.cry](cpp/saw/SDEP_cpp.cry) returns `[20][8]` which
is the in-memory byte layout the IR writes; the proof should go through
with no other changes on this side.
