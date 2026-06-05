# Proposal: Rust cleanup-funclet support in `llvm-exception-lower`

**To:** exception-lowering pass team
**From:** saw-spec-gen / formal-verification
**Date:** 2026-06-05
**Status:** Request for a new mode in the existing pass (not a new repo)

## Problem

SAW's `llvm-pretty-bc-parser` aborts with *"not implemented FUNC_CODE_OPERAND_BUNDLE"* when it hits
Win64 SEH funclet exception handling. On `x86_64-pc-windows-msvc`, **rustc** emits exactly this form
for `Drop` glue, `panic`, and `catch_unwind`: `cleanuppad` / `cleanupret` (+ occasional
`catchswitch` / `catchpad`) with `[ "funclet"(token %cp) ]` operand bundles on the calls inside them.

This blocks SAW from *parsing* any Rust function whose call graph transitively reaches `String`,
`Vec`, `format!`, `HashMap`, or anything else with drop glue — before any override or simulation can
run. Overrides replace semantics at simulation time; they cannot help, because the bitcode never
parses in the first place. The fix must happen at the **IR level, before `llvm_load_module`**.

## Why this belongs in `llvm-exception-lower` (not a new repo)

The pass already lowers the *same construct*. Both clang and rustc emit identical Win64 SEH funclet
IR under `__CxxFrameHandler3`; the pass operates on LLVM IR and is source-language agnostic. Rust's
case is a **strict subset** of what you already handle:

- Rust funclets are overwhelmingly **cleanup** pads (destructor/drop glue during unwind) — structurally
  the same as C++ destructor cleanup the pass already lowers.
- Rust rarely *catches*, so the hard part (C++ `??_R0…` typeinfo catch-dispatch) is usually absent.

A separate repo would duplicate the cmake + LLVM build matrix, the release pipeline, and the
LLVM-version pinning, all to ship a pass that links the same `libLLVM`. saw-spec-gen already installs
**one** binary via `install-exception-lower.ps1` / `ensure-exception-lower.ps1`. Keep it one binary.

## What to build

Add a **cleanup-only lowering mode**:

1. Convert `invoke … to %ok unwind %pad` → `call …` + `br %ok` (drop the unwind edge).
2. Strip `[ "funclet"(token …) ]` operand bundles from calls.
3. Delete `cleanuppad` / `cleanupret` blocks (in the panic-free verified path they never execute).
4. **Do not** attempt C++ typeinfo catch-dispatch for these — there is none.

Selection:

- Prefer **auto-detect**: a function with only `cleanuppad`/`cleanupret` (no `catchpad`/`catchswitch`)
  takes the cleanup-only path automatically. **Do not key off the personality function** — Rust also
  uses `__CxxFrameHandler3` on MSVC, so it does not distinguish Rust from C++.
- Optionally expose an explicit `--mode=cleanup-only` flag for batch pipelines.

## Acceptance criteria

- A `rustc --emit=llvm-bc` fixture with `Drop` glue lowers to funclet-free IR that
  `saw-script` `llvm_load_module` parses without `FUNC_CODE_OPERAND_BUNDLE`.
- A `catch_unwind` fixture lowers without error (cleanup path; catch may be conservatively
  dropped/abort-modeled).
- Existing C++ catch-dispatch fixtures (e.g. `add_one_multi_catch.cpp`) are unchanged — no regression.
- Add the Rust fixtures to the repo's test suite alongside the C++ ones.

## Caveats to document (carry over from the C++ pass)

- You verify the **lowered** form, not production code — soundness gap; no guaranteed equivalence.
- Lowering removes the **parse** blocker, not the **simulation** cost. For pure decision logic that
  only *might* throw on OOM (assume-no-throw), the result is clean straight-line code and that's
  enough. For functions doing real heap/format/hash work, callers still layer allocator/format
  overrides on top.
- Still out of scope (same as today): derived-class catch, full stack unwinding, nested exceptions.

## Downstream impact

Once this lands, saw-spec-gen's Rust pipeline can call the pass instead of relying on
`opt --passes=internalize,globaldce` to *delete* unreachable funclet-laden std bodies. That lets us
keep std-touching code **in** the verified scope when it must stay reachable, rather than only being
able to verify functions whose std dependencies are dead-code-eliminated. The existing
`patch-llvm-ir --strip-msvc-eh` (which today rewrites only EH *globals*, not bodies) can then be
retired for the Rust path.
