# SAW Handoff: Crucible-LLVM panics on `insertvalue` of struct literal containing `poison`

**Priority**: P2 (blocks all verification of unoptimized Rust async / ADT-returning code from `rustc 1.90`)
**Component**: `crucible-llvm` (vendored in `saw-script`)
**SAW revision**: `67f84b23ddfdf7796a18d94889edbbbc393206ae` (HEAD with uncommitted files)
**Reporter**: saw-spec-gen async demo, 2026-05-20

---

## TL;DR

Crucible-LLVM panics with `Attempting to evaluate poison value` when it encounters

```llvm
%r = insertvalue { i32, i32 } { i32 0, i32 poison }, i32 %v, 1
ret { i32, i32 } %r
```

The result has no poison (the `insertvalue` overwrites the only poison field), but
Crucible eagerly evaluates the literal `{ i32 0, i32 poison }` *before* the
`insertvalue` and panics on the poison element. Per LLVM semantics this should
succeed.

This pattern is emitted **routinely** by `rustc 1.90` for ADT returns from
`Future::poll` impls, async coroutine resume functions, and any function that
returns an enum variant with a known discriminant. As a result SAW cannot verify
unoptimized async Rust today; the only workaround is to run `opt -O2` before
loading the bitcode, which is not always desirable (it can inline away functions
you want to verify directly).

---

## Reproducer

The most reduced repro:

```llvm
; reduced_poison.ll
target triple = "x86_64-pc-windows-msvc"

define { i32, i32 } @poll(i32 %v) {
start:
  %r = insertvalue { i32, i32 } { i32 0, i32 poison }, i32 %v, 1
  ret { i32, i32 } %r
}
```

SAW script:

```saw
m <- llvm_load_module "reduced_poison.bc";

let poll_spec = do {
  v <- llvm_fresh_var "v" (llvm_int 32);
  llvm_execute_func [llvm_term v];
  llvm_return (llvm_term {{ (0 : [32], v) }});
};

llvm_verify m "poll" [] true poll_spec z3;
```

Expected: proof succeeds.
Actual:

```
You have encountered a bug in Crucible's implementation.
%< --------------------------------------------------- 
  Location:  llvmExtensionEval
  Message:   Attempting to evaluate poison value
             Type: BVRepr 32
CallStack (from HasCallStack):
  panic, called at src\Lang\Crucible\Panic.hs:11:9 in crucible-0.9.0.0.99-inplace:Lang.Crucible.Panic
  panic, called at src\Lang\Crucible\LLVM\Eval.hs:109:7 in crucible-llvm-0.9.0.0.99-inplace:Lang.Crucible.LLVM.Eval
%< --------------------------------------------------- 
```

A real-world repro is also attached: a single async Rust function
`async fn add_one(x: u32) -> u32 { ReadyU32(x).await + 1 }` compiled with
unmodified `rustc 1.90` flags (`-C opt-level=0 -C panic=abort` etc.) produces
this LLVM in `<ReadyU32 as Future>::poll`:

```llvm
define { i32, i32 } @<ReadyU32 as Future>::poll(ptr %0, ptr %_cx) {
start:
  %self = alloca [8 x i8], align 8
  store ptr %0, ptr %self, align 8
  %_4   = call align 4 ptr @<Pin<&mut ReadyU32> as Deref>::deref(ptr %self)
  %_3   = load i32, ptr %_4, align 4
  %1    = insertvalue { i32, i32 } { i32 0, i32 poison }, i32 %_3, 1
  ret { i32, i32 } %1
}
```

and identical patterns in the coroutine resume function for the async `add_one`.

Full reproducer in the repo:

- Source: demo/async_rust/add_one_sat.rs
- Generated bitcode (post-rustc, pre-opt): `demo/async_rust/out_async_demo/add_one_sat.bc`
- SAW proof script: `demo/async_rust/out_async_demo/verify_async_real.saw`
- Runner: demo/async_rust/run_async_demo.ps1

Reproduce with:

```powershell
cd c:\Users\ameliapayne\saw-spec-gen
pwsh demo/async_rust/run_async_demo.ps1
# Then in the generated out dir:
cd demo\async_rust\out_async_demo
saw verify_async_real.saw   # panics at -O0
```

---

## Workaround that proved this is the bug

Running `opt -O2` on the same bitcode keeps the *exact same* poison pattern
(both `insertvalue { i32, i32 } { i32 0, i32 poison }, i32 %v, 1` instructions
remain after `-O2`), but Crucible-LLVM now handles them correctly and the proof
runs to completion:

```
$ opt -O2 add_one_sat.bc -o add_one_sat_O2.bc
$ saw verify_async_real.saw
Proof failed.
----------Counterexample----------
  x: 0
----------------------------------
Expected term: add_one_spec x         (= x + 1)
Actual term:   bvAdd 32 (bvNat 32 7) x  (= x + 7)
```

(That counterexample is the intended test of the demo — the source body says
`y + 7`, the Cryptol spec says `x + 1`. The important point is that SAW
*completed symbolic execution*, including stepping through every
`insertvalue { i32, i32 } { i32 0, i32 poison }, ...` in the program.)

So whatever changes between `-O0` and `-O2` for Crucible, it's not the IR — it's
something about how the surrounding instructions are arranged.

---

## Root cause (hypothesis)

Crucible-LLVM's evaluator visits sub-terms of an aggregate literal eagerly. When
asked to translate the `{ i32, i32 } { i32 0, i32 poison }` operand to
`insertvalue`, it walks the struct, hits the second field (`poison`), and calls
`llvmExtensionEval` on it, which panics in `Lang.Crucible.LLVM.Eval.hs:109`.

Per LLVM's poison semantics this is incorrect:

> Most instructions return poison when one of their operands is poison.

> A `poison` value can be the operand of an `insertvalue` or `insertelement`
> instruction; the result has the corresponding field/element replaced by the
> new value, and the rest of the result is the rest of the original aggregate.

i.e. `poison` is only contagious when it is *used* (extracted, loaded, fed to
arithmetic). Constructing an aggregate that contains poison is well-defined;
overwriting the poison slot before any extract is well-defined and produces a
poison-free result.

The reason `-O2` works is presumably that some downstream pass (likely SROA +
instcombine) restructures the code path so that Crucible never has to *eagerly*
evaluate the poison aggregate as an operand — it sees the `insertvalue` first
and goes through a different code path that treats the literal as a target into
which fields are stored.

---

## Suggested fix direction

In `crucible-llvm/src/Lang/Crucible/LLVM/Eval.hs`, the poison case at line ~109
should not panic eagerly. Concretely:

1. Represent struct/array elements lazily, e.g. `Vector (PartExpr (RegValue …))`,
   where a poison element is `Unassigned`/`Err` rather than a forced panic.
2. `insertvalue` overwrites slot N with a real value — it should succeed
   regardless of whether the rest of the aggregate contains poison.
3. `extractvalue` / `load` / arithmetic of a poison element is what should
   trigger the panic (or, better, become a verification subgoal that fails).

A targeted minimal fix is probably:

- In `llvmExtensionEval` (or wherever `Lang.Crucible.LLVM.Eval` constructs
  `RegValue` from a constant struct), wrap each field's evaluation in
  `catch`/`tryEval` and represent poison fields with a sentinel.
- In the `insertvalue` translation, do not force the entire aggregate — only the
  fields that are *not* being overwritten.

Even simpler temporary fix that would unblock everything we've hit: replace the
hard `panic` at `Lang.Crucible.LLVM.Eval.hs:109` with a symbolic "undef"
fresh-var of the appropriate type. This is unsound in general (we'd lose the
ability to *detect* genuine poison-use bugs) but matches what most LLVM
front-ends do today, and SAW currently catches nothing here anyway — it just
crashes.

---

## Why this matters for verification of real Rust

- `rustc 1.90` emits `insertvalue { ... } { i32 disc, T poison }, %val, 1` for
  **every** enum variant construction where the discriminant is a constant and
  the payload is computed (`Poll::Ready(v)`, `Some(v)`, `Ok(v)`, `Err(v)`, etc.).
- All `Future::poll` impls return `Poll<T>` and therefore hit this pattern.
- Async coroutine resume functions return `Poll<Output>` and hit this pattern
  twice (once for the `Pending → Ready(...)` transition and once for the
  terminal panic-on-resume-after-completion path).
- At `-O0` (which is what people use during dev), `rustc` does not constant-fold
  the literal away.

Net effect: today SAW cannot verify any unoptimized Rust function that returns
an enum at all. The async demo just happened to make this visible because the
state machine has multiple `insertvalue`-into-poison-literal sites and each one
panics.

---

## Test plan for the fix

1. The minimal IR repro above should verify.
2. `demo/async_rust/run_async_demo.ps1` with `+ 1` in the source should
   produce `Proof succeeded`.
3. `demo/async_rust/run_async_demo.ps1` with `+ 7` in the source should
   produce a counterexample with `x: 0` (or any other input) and
   `Actual term: bvAdd 32 (bvNat 32 7) x`.
4. Existing C++ checksum / adversarial demos should continue to pass —
   they don't hit this pattern, so they're a regression baseline.

---

## Files attached to this handoff

In the saw-spec-gen repo, branch `master`:

- demo/async_rust/add_one_sat.rs — async Rust source (single fn)
- demo/async_rust/add_one_spec.cry — Cryptol spec
- demo/async_rust/run_async_demo.ps1 — driver: rustc → llvm-dis → saw-spec-gen → SAW
- `demo/async_rust/out_async_demo/add_one_sat.bc` — `-O0` bitcode (crashes)
- `demo/async_rust/out_async_demo/add_one_sat_O2.bc` — `-O2` bitcode (works)
- `demo/async_rust/out_async_demo/verify_async_real.saw` — proof script targeting the real coroutine resume

To rebuild artifacts from scratch: `pwsh demo/async_rust/run_async_demo.ps1`.
