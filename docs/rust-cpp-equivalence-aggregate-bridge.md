# Rust ↔ C++ shared-spec equivalence — the aggregate/struct ABI bridge

**To:** saw-spec-gen maintainers
**From:** formal-verification (demo_protocol / SDEP)
**Date:** 2026-06-06
**Status:** Proposed — follow-up to `rust-cpp-equivalence-parity.md`

## TL;DR

`saw-spec-gen` **already** provides most of the plumbing to autoverify
C++ ≡ Rust against a *single* Cryptol spec — for **scalar** functions.
It does **not yet** provide the plumbing for **aggregate / struct / sret**
returns, which is what the real protocol surface needs. Until that gap
closes, a single canonical `SDEP.cry` cannot drive both languages, and
downstream projects are forced to keep two hand-written restatement specs
(`SDEP_cpp.cry`, `SDEP_rust.cry`) plus hand-written `.saw` for every
function with a packed or sret return.

## What already works (no action needed)

Re-verified against current `main`:

| Capability | Where | State |
|---|---|---|
| One `-CryptolFn` drives **both** sides | `verify-equiv.ps1` | ✅ works e2e for scalars (`add_one`) |
| `gen-verify-rust` accepts sret / aggregate / bool / `range` | `gen_verify_rust.rs::resolve_target` | ✅ no longer scalar-only |
| `--variant-map PARAM=V:D,…` (enum subset) | `cli.rs`, `main.rs` | ✅ flag parsed + threaded |
| Scalar ABI width bridge | `saw_emit::cryptol_bridge` | ✅ `BitExtract`/`BitPack` (`i1↔Bit`), `Truncate`/`ZeroExtend` (`[2]↔i8`) |

This closes gaps **(1)** and **(3)** from `rust-cpp-equivalence-parity.md`
and part of **(2)** (the scalar slice of the width bridge).

## What still needs to be handled

The `AbiParamBridge` / `AbiReturnBridge` enums in `cryptol_bridge.rs`
cover **scalars only**. Every remaining SDEP function that isn't a flat
`(iN,…)→iN` still requires a per-language restatement of the spec and/or
a hand-written `.saw`. The missing adapters, with concrete SDEP cases:

### A. Packed aggregate returns (register-returned small structs)

Canonical spec returns a tuple/record; each ABI packs it differently.

- **`enforceAccess : AccessMode -> AccessDecision -> (Bit, Bit)`**
  - C++ (MSVC small-struct-in-register): return is `i16` packing
    `{i8 allowed, i8 logged}` little-endian.
  - Rust: return is an `{i1, i1}` LLVM struct value.

  Needed: `AbiReturnBridge` variants that lower a Cryptol tuple/record to
  (a) a packed `iN` (`(a # b)` / explicit byte-place with endianness) and
  (b) an `llvm_struct_value [...]`. Today neither exists, so `enforceAccess`
  is hand-written on both sides.

### B. Struct ↔ byte-buffer serialization for sret

- **`getStatus : … -> StatusStruct` → 20-byte sret buffer.**
  - Canonical spec yields a structured value; the ABI is a 20-byte
    `llvm_points_to` layout with field offsets.
  - C++ does a **read-modify-write** (`preBytes` in, mutated bytes out);
    Rust **writes all 20** bytes.

  Needed: a struct→bytes serializer driven by the spec's field layout, plus
  a way to declare "these byte ranges are preserved from prestate"
  (the C++ `preBytes` case) vs "fully written" (Rust). The sret *plumbing*
  exists (`sret_prestate.rs`), but the spec-side **value bridge** (Cryptol
  record → concrete byte vector at ABI offsets) does not.

### C. Niche-packed enum discriminant ↔ wide enum

- **`ActivationResult`**: spec/C++ = 3 variants (`i8`,
  `{Success,AlreadyActive,IoFailure}`); Rust = 2 variants niche-packed to
  `i1` (`{Success,AlreadyActive}`).

  `--variant-map` already restricts the reachable discriminants. What's
  missing is the **composition** of variant-map with the width bridge so the
  return adapter maps Rust's `i1` bit pattern → the spec's `i8` discriminant
  value (niche bit pattern ≠ discriminant value in general). For SDEP the
  niche is identity (0/1), but the generator should emit the discriminant
  remap rather than relying on that coincidence.

### D. Endianness / field order for multi-field packing

Any of the above that packs ≥2 fields into one integer (case A) must emit a
defined byte/bit order. C++ MSVC packs little-endian; the generator needs to
encode this so the proof is sound, not by-eye.

## Concretely, the asks

1. Extend `AbiReturnBridge` (and a matching param side, for struct *inputs*)
   with **aggregate variants**:
   - `PackInt { fields: Vec<(offset_bits, width)>, endian }` → Cryptol
     `(... # ...)` packed `iN`.
   - `StructValue { fields }` → `llvm_struct_value [...]`.
   - `StructBytes { fields: Vec<(byte_offset, width)>, preserved: Vec<range> }`
     → byte-vector for sret, with preserved ranges threaded from prestate.
2. Compose `--variant-map` with the width/discriminant bridge so a niche /
   subset enum return remaps to the canonical discriminant width.
3. Route all of the above through `saw_emit::cryptol_bridge` so **both**
   `gen-verify` and `gen-verify-rust` consume the **same** `SDEP.cry`
   function — deleting `SDEP_cpp.cry` and `SDEP_rust.cry`.

## Acceptance check

Extend the `verify-equiv.ps1` e2e beyond `add_one` with three cases sharing
**one** `SDEP.cry`-style spec, **no** per-language restatement, **no**
hand-written `.saw`, each ending in `EQUIVALENT`:

- a packed tuple return (`enforceAccess`-shaped: C++ `i16`, Rust `{i1,i1}`);
- an sret struct with a preserved-bytes prestate (`getStatus`-shaped);
- a niche-packed subset enum return + `--variant-map`
  (`ActivationResult`-shaped: C++ `i8`/3-variant, Rust `i1`/2-variant).
