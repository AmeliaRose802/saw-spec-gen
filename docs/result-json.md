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
| `reason_code`    | string \| null                                    | yes      | Machine-readable reason for the verdict.  `null` for `VERIFIED`.  See [Reason codes](#reason-codes) below. |

All optional consumer fields are emitted as `null` (or `[]` for
`counterexample`) rather than omitted, so the keyset is stable.

### Verdict semantics

| `verdict`        | Meaning                                                   |
|------------------|-----------------------------------------------------------|
| `VERIFIED`       | SAW proved the implementation matches `cryptol_fn` on every input. |
| `DISPROVED`      | SAW returned a counterexample (recorded in `counterexample`). |
| `UNKNOWN`        | SAW returned neither `VERIFIED` nor a counterexample (timeout, parser error, internal simulation error, etc.). |
| `EQUIVALENT`     | (`side="equiv"` only) both C++ and Rust sides individually `VERIFIED`. |
| `NOT EQUIVALENT` | (`side="equiv"` only) at least one side disagreed with `cryptol_fn`. |

### Reason codes

`reason_code` disambiguates `UNKNOWN` and `DISPROVED` verdicts for downstream tooling.

| `reason_code`                    | Verdict(s)        | Meaning |
|----------------------------------|-------------------|---------|
| `disproved_counterexample`       | `DISPROVED`       | SAW produced a genuine semantic counterexample to the proof obligation. |
| `unknown_internal_memory_error`  | `UNKNOWN`         | SAW reported an internal simulation or memory-load error (e.g. unresolved `std::optional` accessor under symbolic execution).  Not a model mismatch. |
| *(null)*                         | `VERIFIED`        | Proof succeeded; no reason code needed. |
| *(null)*                         | `UNKNOWN`         | Generic unknown — SAW returned neither `VERIFIED` nor a counterexample (timeout, parser error, etc.). |

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
  "impl_file": "add_one_verified.cpp"
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

All three wrappers source the shared helper
[`scripts/Write-ResultJson.ps1`](../scripts/Write-ResultJson.ps1):

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
[`Write-ResultJson.ps1`](../scripts/Write-ResultJson.ps1) and this
document.  Anything that changes the meaning of an existing field
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
