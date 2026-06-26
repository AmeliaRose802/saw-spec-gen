# One-pager: config and inference over CLI flags

**Tool:** saw-spec-gen · **Status:** Part 1 (per-function config table)
implemented; Part 2 (inference) — max-len + `-O0`→`-O1` auto-retry
implemented, variant-map deferred (unsound to auto-derive) ·
**Motivating gap:**
demo_protocol `cpp/saw/run.ps1` hand-codes per-function `ExtraArgs` arrays

## The gap in one sentence

Per-function spec shaping still lives in **CLI flags** baked into driver scripts,
which is invisible in version control and easy to get wrong — it should live in a
**versioned `saw-spec-gen.toml`** (per-function) or be **inferred**, continuing
the trajectory the project already set (`--bind-cryptol-lengths` removed in
favor of inference; `--container-layouts` / `container_layouts.toml` slated for
deletion in favor of AST-derived layouts).

## Evidence

`canonicalize_lp` in the demo driver needs six flags every run:

```powershell
ExtraArgs = @(
  '--in-buffer-size','m=4', '--in-buffer-size','b=4',
  '--out-buffer-param','out=10', '--cryptol-fn-out','out=canonicalize_lp_post',
  '--max-len-precond','nm=4', '--max-len-precond','nb=4')
```

None of this is discoverable from the repo without reading a PowerShell array,
and a typo silently changes what was proved.

## What's already config vs. still flag-only

`saw-spec-gen.toml` (`src/project_config.rs`) already covers, but **globally
only**: `no_struct_shape_recognizer`, `use_llvm_combine_modules`,
`spec_only_on_missing`, `alias_size`, `alias_enum`, `in_buffer_size`,
`max_len_precond`.

Still **CLI-only**, and inherently **per-function**:
`out-buffer-param`, `cryptol-fn-out`, `cryptol-fn-pre`, `cryptol-arg-order`,
`variant-map`. Plus the human-chosen `-O0`/`-O1` bitcode toggle in the driver.

## Proposal

### 1. Per-function config table

Add a `[functions.<cryptol_fn>]` table to `ProjectConfig` (config is global-only
today). Keyed by Cryptol fn (or impl symbol). The `canonicalize_lp` block becomes:

```toml
[functions.canonicalize_lp]
in_buffer_size   = ["m=4", "b=4"]
out_buffer_param = ["out=10"]
cryptol_fn_out   = ["out=canonicalize_lp_post"]
max_len_precond  = ["nm=4", "nb=4"]
```

Extend `MergedConfig` / `apply()` so resolution is **per-function config →
global config → CLI override** (CLI stays only as an ad-hoc escape hatch, never
required). Migrate the five per-function flags above into both global and
per-function tables.

> **Implemented (PR: config-over-cli-flags).** `FunctionConfig` +
> `ProjectConfig.functions: HashMap<String, FunctionConfig>` carry the
> per-function tables. `apply(function, CliFlags)` now resolves
> per-function → global → CLI for every shaping `Vec` (`alias_size`,
> `alias_enum`, `in_buffer_size`, `max_len_precond`, `out_buffer_param`,
> `cryptol_fn_out`, `cryptol_fn_pre`, `cryptol_arg_order`, `variant_map`)
> and the booleans (`no_struct_shape_recognizer`,
> `use_llvm_combine_modules`, `spec_only_on_missing`). `out_buffer_param`,
> `cryptol_fn_out`, `cryptol_fn_pre`, `cryptol_arg_order`, and
> `variant_map` are now config-backed (global *and* per-function), not
> CLI-only. Unit tests cover TOML parse + the three-layer merge.

### 2. Infer what is derivable (preferred over declaring)

Continue the inference-first direction already in the codebase:

* **Buffer / max-len from Cryptol widths.** A Cryptol arg typed `[n][8]` already
  pins its length; lengths are *already* inferred since `--bind-cryptol-lengths`
  was removed. Extend the same inference so `in_buffer_size` / `max_len_precond`
  rarely need to be written at all.

  > **Implemented (PR: config-over-cli-flags).**
  > `length_binding::infer_len_preconds` reuses the existing `bind_lengths`
  > pass: for every struct-shape-recognized `(buf, len)` pair whose buffer
  > carries a synthetic `InReadsParam` annotation, it reads the Cryptol upper
  > bound `n <= K` and emits `(len, K)`. `array_view_passes::apply_inferred_len_preconds`
  > merges those into `BufferOverrides::max_len_preconds` (CLI/config entries
  > win on name collisions); `gen_verify::run` wires it in after the
  > length-binding pass. Open-ended bounds (no `<= K`) are skipped — there is
  > no sound `K` to assert. So `--max-len-precond` is now only needed when the
  > Cryptol signature does *not* already pin the buffer width.

* **`-O0` → `-O1` auto-retry.** The only reason the driver sets `Bc='O1'` for
  `getStatus` is that `std::optional` ctors fail to simulate at `-O0`. Have
  `verify-cpp` (`compile.rs`) detect the empty-struct-global load failure and
  transparently retry the target at `-O1`, removing the human toggle.

  > **Implemented (PR: config-over-cli-flags).** `emit_bitcode` / `emit_llvm_ir`
  > now take an explicit opt level. `verify_cpp::run` runs SAW once at `-O0`;
  > if the result is *non-conclusive* (no `VERIFIED`, no `Counterexample`) and
  > the transcript matches `is_empty_struct_load_failure`, it calls
  > `compile::recompile_at_o1` (re-emit bitcode + IR at `-O1`, re-lower
  > exceptions, re-patch, regenerate the verify script) exactly once and
  > re-runs SAW. The retry is gated on a non-conclusive verdict, so a genuine
  > `VERIFIED`/`DISPROVED` is never overridden; a false-positive match merely
  > costs one extra `-O1` attempt. The `Bc='O1'` driver toggle becomes
  > unnecessary.

* **`variant-map` from enum width parity.** Derive the discriminant remap from
  the clang-AST enum bit-width vs. the Cryptol variant width, instead of hand
  listing `PARAM=V1:D1,V2:D2`.

  > **Deferred (tracked separately).** Auto-deriving the discriminant remap is
  > **unsound** without validation: a Rust enum's in-memory tag uses niche /
  > layout-optimized bit patterns that do *not* equal the source discriminant
  > value, so a silently-synthesized `--variant-map` would verify the impl
  > against the *wrong* Cryptol model. Width parity alone (enum bit-width ==
  > Cryptol variant width) cannot distinguish a correct mapping from a wrong
  > one, and the Rust return path does not yet plumb the per-variant niche
  > layout needed to emit a *correct* map. Rather than ship an unsound
  > auto-apply (or a width-only diagnostic that would mostly fire as noise),
  > this bullet stays manual via the config-backed `variant_map` key from
  > Part 1. Revisit once enum niche layout is surfaced from MIR.

## End state

The demo driver's target table collapses to `(cpp_fn, cryptol_fn)` pairs; every
shaping decision is either inferred or sitting in a reviewable
`saw-spec-gen.toml`. Adding a new verified function no longer means editing
`run.ps1` — it means adding a `[functions.*]` block (or nothing, if inferable).
This matches the philosophy already written into `container_layouts.toml`'s
deletion notice: *users should not hand-encode ABI/shaping details the tool can
derive.*

## Acceptance

`canonicalize_lp` (and the `-O1` `getStatus` case) verify with **no `ExtraArgs`
in `run.ps1`** — all shaping comes from `saw-spec-gen.toml` or inference, and the
config diffs cleanly in git.
