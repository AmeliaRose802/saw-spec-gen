# Rust ↔ C++ shared-spec equivalence — parity gaps in `gen-verify-rust`

## Context

`verify-equiv.ps1` already proves that a **C++** function and a **Rust**
function both satisfy the **same** hand-written Cryptol spec (and are
therefore equivalent to each other). It works today for scalar targets
— the e2e `add_one` case, signature `(iN, …) -> iN`.

The blocker for using it on real protocol code (e.g. the SDEP demo:
`provision_key`, `enroll_device`, `enforce_access`, `get_status`, …) is
that the two sides are **not at feature parity**:

| Pipeline stage | C++ (`gen-verify`) | Rust (`gen-verify-rust`) |
|---|---|---|
| Source of spec | `gen_verify.rs` | `gen_verify_rust.rs` |
| Accepted signatures | structs, pointers, sret, scalars | **integer-only `(iN,…)→iN`** |
| sret struct returns | ✅ `sret_prestate.rs` | ❌ `resolve_target` bails |
| register/packed returns (`i16`, `{i1,i1}`) | ✅ | ❌ bails |
| `llvm_precond` from `range(N,M)` attrs | ✅ | ❌ none |
| buffer overrides (`--in-buffer-size`, `--out-buffer-param`, `--cryptol-fn-out`, `--max-len-precond`, `--cryptol-arg-order`) | ✅ | ❌ none |
| `--spec-only-on-missing` | ✅ | ❌ none |
| alias/enum size overrides | ✅ | ❌ none |

Because of this, downstream projects that want a *single* Cryptol spec
per function are forced to (a) maintain a **second** Rust-ABI restatement
of the spec, and (b) **hand-write** the Rust `verify.saw` for any function
with a struct/sret return or a `range` precondition. The C++ side needs
neither.

CLI definitions for both subcommands: `src/main.rs` L36–88.
The Rust resolver that rejects non-scalar signatures:
`src/gen_verify_rust.rs::resolve_target` (filters on `type_int_bits(..)`
being `Some` for **every** param and the return).

---

## Changes requested on the `saw-spec-gen` side

### 1. Bring `gen-verify-rust` to signature parity with `gen-verify`

`resolve_target` currently discards any candidate whose params or return
aren't `iN`. Generalize it (or fork the C++ emitters) to cover the same
ABI shapes the C++ path already handles. Concretely:

- **sret returns** — when the first LLVM param is `ptr sret(%T)`,
  allocate the out buffer, drop it from the Cryptol arg list, and pin
  the post-state with `llvm_points_to`. The C++ side already does this in
  `sret_prestate.rs`; ideally share that module rather than reimplement.
- **register/packed aggregate returns** — `{i1,i1}` (Rust two-field
  struct) and `iN`-packed structs (MSVC small-struct-in-register, e.g.
  `i16` for `{i8,i8}`). Emit `llvm_struct_value [...]` or the packed-int
  construction on the Cryptol side, matching `llvm_return.rs` /
  `verify_script_steps.rs`.
- **`llvm_precond` from LLVM range attributes** — Rust enums lower with a
  `range(0, N)` parameter attribute (and `!range` metadata on returns).
  Parse it and emit `llvm_precond {{ arg <= N-1 }}` automatically, so the
  caller doesn't hand-write `vault_result <= 2`.
- **buffer-override flags** — forward the same five flags the C++
  `GenVerify` arm accepts (`--in-buffer-size`, `--out-buffer-param`,
  `--cryptol-fn-out`, `--max-len-precond`, `--cryptol-arg-order`) so
  bounded-buffer functions (canonicalize-style) work identically.
- **`--spec-only-on-missing`** — for Cryptol-only helpers with no Rust
  symbol, soft-exit with a `result.json` `status=not_attempted` instead
  of erroring, matching `emit_spec_only_result` on the C++ path.

**Outcome:** the Rust generator emits the full `verify.saw` for every
SDEP function with **zero** hand-written SAW, exactly like C++.

### 2. ABI-adapter layer so ONE Cryptol fn drives both languages

Today equivalence requires the Cryptol fn to *already* be at each target's
exact ABI width — which forces two restatement modules (`SDEP_cpp.cry`,
`SDEP_rust.cry`) that are structurally identical to the canonical
`SDEP.cry` but differ in widths/packing.

Have the generator consume `(canonical Cryptol fn, target ABI descriptor)`
and emit the **width/packing bridge** itself on each side:

- `Bit ↔ i1` (the existing `(arg ! 0)` bridge — generalize it)
- 2-/3-bit enum `↔ i8` (zext/trunc)
- niche-packed enum `↔ i8` discriminant
- little-endian struct/field packing for register returns

With this, both `gen-verify` and `gen-verify-rust` reference the **same**
`SDEP.cry` function, and the two restatement `.cry` files disappear.
The bridge belongs in `saw_emit::cryptol_bridge` (already the shared
type-bridge module both generators route through).

### 3. Variant-count mismatch: enum-subset / variant-map annotation

A real-world wrinkle: an impl may expose **fewer** enum variants than the
canonical spec. In SDEP, Rust `ActivationResult = {Success, AlreadyActive}`
(2 variants, niche-packed to `i1`) while the spec/C++ has
`{Success, AlreadyActive, IoFailure}` (3 variants, `i8`).

A single shared spec then can't be checked verbatim at the Rust return
width. Add a way to express “**this impl realizes a subset of the spec’s
variants**”, e.g. a `--variant-map ActivationResult=Success:0,AlreadyActive:1`
flag (or a sidecar JSON) that the generator uses to:

- restrict the precondition to the reachable discriminants, and
- emit the narrowing adapter (`i1 ↔ i8` discriminant) for the return.

The proof then reads as “Rust matches the spec **restricted to the
variants the Rust API can produce**”, which is the honest statement.

---

## Suggested sequencing

1. **(1) first** — it’s self-contained, unblocks auto-generated Rust
   `verify.saw` for struct/sret/precond functions, and immediately lets
   `verify-equiv.ps1` run on the full SDEP surface (still against a
   Rust-width spec).
2. **(2)** — collapses the two restatement specs into one; biggest
   maintenance win.
3. **(3)** — only needed where impl/spec variant counts diverge; can be
   a follow-up.

## Acceptance check

An e2e case mirroring `add_one` but with **(a)** an sret struct return,
**(b)** a `range`-constrained enum param, and **(c)** a shared
`SDEP.cry`-style spec consumed by *both* sides with no per-language
restatement — verified end-to-end by `verify-equiv.ps1` → `EQUIVALENT`.
