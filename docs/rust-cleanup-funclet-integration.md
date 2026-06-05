# Integration: wire the Rust cleanup-funclet lowering into saw-spec-gen

**To:** saw-spec-gen maintainers
**From:** formal-verification
**Date:** 2026-06-05
**Status:** Ready to implement — `llvm-exception-lower` cleanup-only mode has landed

## Goal

The exception-lowering pass now lowers rustc's Win64 SEH cleanup funclets (`cleanuppad` /
`cleanupret` + `[ "funclet"(token …) ]` bundles) to funclet-free IR. Make the Rust pipeline **use
it unconditionally**, the same way the C++ pipeline already does. This replaces the
`-C panic=abort` dodge — **no backwards-compatible fast lane, no opt-in switch.** One path: compile
with unwinding, lower, verify.

## Why drop `panic=abort`

`panic=abort` was only ever a way to *avoid emitting funclets* so SAW's parser wouldn't choke. It is
unsound as a default: it verifies a binary with **different unwind semantics** than the shipped
`panic=unwind` artifact, and it forces `internalize,globaldce` to *delete* any std-touching drop glue
just to keep the module parseable — which excludes that code from the verified scope. With the pass
in place both hacks are obsolete. Keeping two code paths only invites the abort lane to silently
verify the wrong thing.

## Changes

1. **`verify-rust.ps1` — switch the compile.** Replace `-C panic=abort` (L130, and the harness
   build at L334) with `-C panic=unwind`. Remove the comment block justifying abort.

2. **`verify-rust.ps1` — insert lowering (new Step 1.25).** After rustc emits the `.bc` and before
   `patch-llvm-ir` / `opt` / `llvm_load_module`, call the lowering pass. Reuse the C++ runner: call
   `scripts/ensure-exception-lower.ps1` with the Rust `.bc`. It already runs
   `& $ExceptionLower $BcFile -o $loweredBc` and copies back in-place — mechanically language-agnostic.

3. **`scripts/ensure-exception-lower.ps1` — generalize.** Drop the "C++"-specific log strings and the
   `$IsMsvc`-gated "try/catch demos will not load" note; make the messages language-neutral. Add a
   `-Mode` param defaulting to the pass's auto-detect; pass `--mode=cleanup-only` through **only if**
   the pass requires it (auto-detect on a cleanup-only function should be sufficient — it has no
   `catchpad`/`catchswitch`).

4. **`discover-tools.ps1` — no change.** `$tools.ExceptionLower` is already discovered generically.

5. **`scripts/install-exception-lower.*` — refresh the pin.** The build on disk is the old C++-only
   binary (`OVERVIEW: LLVM C++ exception-lowering pass`, no cleanup flag). Default `ReleaseTag` is
   `latest-main`, but install no-ops when a binary exists — delete `~/.saw-spec-gen/exception-lower`
   (or run with `-Force`) so the cleanup-capable build lands. Bump any pinned tag to the release that
   includes cleanup mode.

6. **Rust emit path — confirm globals.** `gen_verify_rust` does **not** call `inject_exclow_globals`
   (only `gen_verify.rs:198` does). Cleanup-only lowering deletes cleanup pads without synthesizing
   `@__exclow_error_*` globals, so this should stay correct. Verify once on the new binary: lower a
   `panic=unwind` Drop fixture and grep the output `.ll` for `@__exclow_`. If any appear, add the
   same `inject_exclow_globals` call to the Rust path.

7. **`patch-llvm-ir` — retire the Rust EH workaround.** The `--strip-msvc-eh` global rewrite is no
   longer load-bearing for Rust once the pass runs; keep it as a harmless no-op or remove its Rust
   invocation. Stop relying on `internalize,globaldce` to delete funclet-laden bodies for parseability
   (it may still run for scope control, but not as the EH workaround).

## Tests

- Recompile the adversarial drop fixtures (`drop_noinline`, `drop_side_effect`) with `-C panic=unwind`
  and assert their emitted `.ll` now **contains** `cleanuppad` pre-lowering and **none** post-lowering,
  then `VERIFIED` end-to-end. These currently pass only because abort emits no funclets — they must
  now exercise the real lowering.
- Add one `catch_unwind` fixture: lowers without `FUNC_CODE_OPERAND_BUNDLE`, cleanup path verified
  (catch conservatively abort-modeled).
- Remove every `-C panic=abort` line from `tests/e2e/cases/11-rust-parity/**/Check-*.ps1` and the
  research cases; the suite must be green with `panic=unwind` only.
- C++ exception fixtures (`tests/e2e/cases/06-throws-exception`) must be unchanged — no regression.

## Acceptance

- `verify-rust.ps1` compiles with `panic=unwind`, lowers, and verifies the demo's std-touching Rust
  with no `internalize,globaldce` EH workaround and no `panic=abort` anywhere in the repo.
- `grep -r 'panic=abort' .` returns nothing under `verify-rust.ps1`, `scripts/`, and `tests/e2e/`.
- Full e2e suite green; C++ suite unchanged.
