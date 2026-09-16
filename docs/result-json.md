# `result.json` — per-run verification record

Every invocation of `verify.ps1`, `saw-spec-gen verify-rust`
(`verify-rust.ps1` is now a shim to that subcommand), or
`verify-equiv.ps1` writes a single `result.json` into the run's output
directory. The file is the machine-readable contract between SAW
verification and downstream tooling (the e2e runner, the
`saw-spec-gen collect-results` adapter, `pretty-specs` docs badges).

This document describes **schema version `1`**.

## Location

```
<output-dir>/result.json
```

For `verify.ps1` / `saw-spec-gen verify-rust`, `<output-dir>` defaults to
`out_<basename>/` next to the source file (or whatever
`-OutputDir <path>` overrides).  `verify-equiv.ps1` writes one file at
`<output-dir>/result.json` (the combined verdict) plus per-side files
at `<output-dir>/cpp/result.json` and `<output-dir>/rust/result.json`.

## Schema

| Field            | Type                                              | Required | Notes |
|------------------|---------------------------------------------------|----------|-------|
| `schema_version` | string                                            | yes      | `"1"` for this revision; consumers must reject unknown values. |
| `side`           | `"cpp" \| "rust" \| "equiv"`                      | yes      | Which wrapper produced the file. |
| `function`       | string                                            | yes      | Implementation function name (unmangled). |
| `cpp_function`   | string                                            | no       | `side="equiv"` only: C++ symbol name passed to `verify.ps1`. |
| `rust_function`  | string                                            | no       | `side="equiv"` only: Rust symbol name passed to `verify-rust.ps1`. |
| `cryptol_fn`     | string                                            | yes      | Cryptol spec function checked against. |
| `verdict`        | `"VERIFIED" \| "DISPROVED" \| "UNKNOWN" \| "EQUIVALENT" \| "NOT EQUIVALENT"` | yes | `EQUIVALENT` / `NOT EQUIVALENT` are only emitted by `side="equiv"`. |
| `counterexample` | array of `{name, value, bits?}`                   | yes      | Empty `[]` for `VERIFIED`/`UNKNOWN`/`EQUIVALENT`.  `name` and `value` are strings; optional `bits` is an integer (LLVM bit width). |
| `expected`       | string \| null                                    | yes      | Cryptol spec value evaluated at the counterexample inputs (string-encoded integer).  `null` when no counterexample. |
| `actual`         | string \| null                                    | yes      | Implementation value (recompile-and-run) at the counterexample inputs.  `null` when no counterexample or recompile failed. |
| `solver`         | string \| null                                    | yes      | Solver SAW dispatched to (currently always `"z3"` when set). |
| `time_secs`      | number \| null                                    | yes      | Wall-clock seconds the SAW invocation took, when measured. |
| `impl_file`      | string \| null                                    | yes      | Source file basename (the `.cpp` / `.rs` that produced the bitcode/MIR).  For `side="equiv"`, both basenames joined with `" | "`. |
| `contract`       | object                                            | C++      | Unified implementation-function contract. Its `clauses` array records every checked return or memory assertion and the Cryptol term from which that clause came. |
| `memory_layout`  | object                                            | no       | Native C++ checked compiler-layout plan, when available. Omitted otherwise; not a separate proof verdict. |

Each `contract.clauses` entry has this shape:

| Field        | Type           | Meaning |
|--------------|----------------|---------|
| `name`       | string         | Clause name: `"return"` or the mutated region name. |
| `assertion`  | string         | SAW assertion receiving the clause: `"llvm_return"` or `"llvm_points_to"`. |
| `region`     | string \| null | Mutated memory region, or `null` for the return clause. |
| `cryptol_fn` | string         | Top-level Cryptol function supplying this clause. |
| `projection` | string \| null | Record field projected from `cryptol_fn`, or `null` for legacy direct-return functions. |

The C++ implementation function still appears exactly once in the top-level
`function` field and receives one verdict for the conjunction of all clauses.
Legacy split syntax remains accepted: for example, a return model `activateRet`
plus `cryptol_fn_out = ["this=activatePost"]` is represented as one contract
whose return clause has provenance `activateRet` and whose `this` clause has
provenance `activatePost`. The two Cryptol helpers are not independent proof
subjects.

All optional consumer fields are emitted as `null` (or `[]` for
`counterexample`) rather than omitted, except `memory_layout`, which is omitted
when no checked C++ plan is available.

### Compiler-derived memory layout (C++)

Native `verify-cpp` embeds the checked plan in top-level `memory_layout`.
See [29-compiler-derived-object-layouts.md](29-compiler-derived-object-layouts.md)
for configuration, supported layouts and proof boundaries.

| Nested field | Meaning |
|---|---|
| `schema_version` | Integer `1`, distinct from the outer result schema string `"1"`. |
| `target_triple`, `data_layout`, `compiler`, `abi`, `command` | Compiler/target provenance; `abi` is `msvc` or `itanium`. |
| `objects` | Region-keyed map, including `return` for sret; not an array. |
| `abstract_objects` | Layouts for eligible abstract vptr-only interface parameters using explicit assumed contracts; no complete derived-object coverage claim. |
| `records`, `llvm_types` | Full captured record trees and named LLVM definition snapshot. The compiler sidecar additionally retains `irgen_types`; the plan does not duplicate that map. |
| `validity_constraints`, `semantic_preconditions` | Recorded representation/selection constraints and original user preconditions, respectively; user restrictions are not inferred C++ validity. |
| `warnings`, `abstraction_boundaries` | Allocation assumptions, other scope warnings and remaining runtime/callee abstraction boundaries. Also inspect per-object validation notes. |

Each `objects.<region>` entry contains `region`, `projection`, `mutable`,
zero-based `argument_index`, `lowering` (`pointer`, `byval`,
`indirect_by_value` or `sret`), `configured_shape`, `inferred_shape`,
`asserted`, `framed`, `selectors` and `layout`.
The layout includes `source_type`, `llvm_type`, `allocation_type`, `size`,
`alignment`, `llvm_alignment`, `fields`, `padding`, `bases`, `validation` and
`unresolved`. Field entries record relative paths, source/LLVM types, extents,
optional bit/array metadata, guards, validity, and pointer/runtime markers.
`allocation_type` identifies a named alias or the exact inline compiler IRgen
storage used by SAW.

`contract.clauses` still identifies logical return/state clause provenance;
`memory_layout` explains sret lowering and the typed, guarded field assertions.
Padding is not a semantic return field. Frames check unchanged **final values**,
not absence of transient writes. A VERIFIED result remains subject to its
preconditions and recorded assumptions; it does not prove concurrency, general
C++ lifetimes or compiler correctness. `REJECTED` is an E2E classification for
pre-proof generation failures, not an additional result-schema verdict.

### Verdict semantics

| `verdict`        | Meaning                                                   |
|------------------|-----------------------------------------------------------|
| `VERIFIED`       | SAW proved the implementation matches the contract on inputs admitted by the preconditions, under the proof's explicit assumptions. |
| `DISPROVED`      | SAW returned a counterexample (recorded in `counterexample`). |
| `UNKNOWN`        | SAW returned neither `VERIFIED` nor a counterexample (timeout, parser error, etc.). |
| `EQUIVALENT`     | (`side="equiv"` only) both C++ and Rust sides individually `VERIFIED`. |
| `NOT EQUIVALENT` | (`side="equiv"` only) at least one side disagreed with `cryptol_fn`. |

### Example — `VERIFIED`

```json
{
  "schema_version": "1",
  "side": "cpp",
  "function": "add_one",
  "cryptol_fn": "add_one_spec",
  "verdict": "VERIFIED",
  "counterexample": [],
  "expected": null,
  "actual": null,
  "solver": "z3",
  "time_secs": null,
  "impl_file": "add_one_verified.cpp",
  "contract": {
    "clauses": [
      {
        "name": "return",
        "assertion": "llvm_return",
        "region": null,
        "cryptol_fn": "add_one_spec",
        "projection": null
      }
    ]
  }
}
```

### Example — `DISPROVED`

```json
{
  "schema_version": "1",
  "side": "rust",
  "function": "compute_fee",
  "cryptol_fn": "compute_fee_spec",
  "verdict": "DISPROVED",
  "counterexample": [
    { "name": "x", "value": "2147483647", "bits": 32 },
    { "name": "rate", "value": "2", "bits": 32 }
  ],
  "expected": "4294967294",
  "actual": "0",
  "solver": "z3",
  "time_secs": null,
  "impl_file": "compute_fee_disproved.rs"
}
```

## Producing this file

Native C++/Rust verification writes through
[the shared Rust writer](../src/verify_result.rs#L1); the C++ writer attaches the
checked layout plan when present. The PowerShell writer is
[the shared result helper](../scripts/Write-ResultJson.ps1#L1):

```powershell
. (Join-Path $ScriptRoot 'scripts/Write-ResultJson.ps1')
Write-VerifyResult `
    -OutputDir      $OutputDir `
    -Side           'cpp' `
    -Function       $Function `
    -CryptolFn      $CryptolFn `
    -Verdict        'DISPROVED' `
    -Counterexample @($cexPairs) `
    -Expected       $expectedVal `
    -Actual         $actualVal `
    -Solver         'z3' `
    -ImplFile       (Split-Path -Leaf $CppFile)
```

Adding a new field to schema `1` requires only updating
the relevant native/PowerShell writers and this document.
Anything that changes the meaning of an existing field
requires bumping `schema_version` and teaching the consumers to handle
both revisions (or to reject the older one with a clear error).

## Consuming this file

The reference consumer is `saw-spec-gen collect-results`, which walks
a directory tree, finds every `result.json`, and emits a single
`proof_manifest.json` for `pretty-specs --proof-status`.  It rejects
files whose `schema_version` it doesn't recognise.

## Per-property results from a single SAW invocation

A single emitted `.saw` script may run multiple `llvm_verify` (or, in
future, `prove_print`) commands.  The emitter wraps each one with the
machine-readable `BEGIN_PROOF` / `PROVED` markers documented in
[`proof-markers.md`](proof-markers.md).
[`scripts/Parse-PropertyLog.ps1`](../scripts/Parse-PropertyLog.ps1)
reads a captured SAW log and writes one schema-1 `result.json` per
property under `<output-dir>/properties/<name>/result.json`, which
`collect-results` then aggregates exactly as if each had come from a
separate wrapper invocation.
