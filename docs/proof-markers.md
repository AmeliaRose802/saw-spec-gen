# SAW proof markers — `BEGIN_PROOF` / `PROVED`

Every verification script emitted by `saw-spec-gen` wraps each
`llvm_verify` (and, in future, each `prove_print`) with a pair of
machine-readable marker lines:

```
print "BEGIN_PROOF <name>";
<verification command>
print "PROVED <name>";
```

This document describes the stable contract those markers expose.
Downstream tooling (`scripts/Parse-PropertyLog.ps1`,
`saw-spec-gen collect-results`, the pretty-specs status badges) keys
on the exact strings below, so the format is part of the public
interface — any change requires a coordinated bump across producers
and consumers.

## Why

SAW's `llvm_verify` and `prove_print` commands are **silent on
success**.  A script that proves 22 properties in a row produces empty
stdout when everything passes; the only signal is the process exit
code.  That makes it impossible to:

- Tell which properties were attempted versus skipped.
- Associate a SAW warning / error line with the specific property
  that triggered it.
- Produce per-property results from a single SAW invocation (see
  `saw-spec-gen-0ft` — multi-property suites).

The markers fix all three: every property gets an opening
`BEGIN_PROOF` line, and a closing `PROVED` line on success.

## Format

Lines are emitted exactly as shown, with literal double-quotes and a
trailing semicolon (the SAWScript `print` syntax):

```
print "BEGIN_PROOF <name>";
print "PROVED <name>";
```

`<name>` is the **source-level identifier** of the property:

| Emit site                             | `<name>` value                         |
|---------------------------------------|----------------------------------------|
| C++ → Cryptol equivalence proof       | the unmangled C++ function name        |
| Rust → Cryptol equivalence proof      | the Rust function name (no mangling)   |
| Future Cryptol property bundle proofs | the Cryptol property identifier        |

The mangled symbol passed to `llvm_verify` is **not** used for the
marker — `<name>` must round-trip through Cryptol / log files where
mangled identifiers are user-hostile.

## Success / failure semantics

| Observed in SAW log                          | Meaning                                                                       |
|----------------------------------------------|-------------------------------------------------------------------------------|
| `BEGIN_PROOF foo` followed by `PROVED foo`   | Property `foo` was proven by SAW (verdict `VERIFIED`).                        |
| `BEGIN_PROOF foo` with **no** matching `PROVED` before EOF | Property `foo` failed.  Any counterexample / error lines after the `BEGIN_PROOF` and before the next `BEGIN_PROOF` (or EOF) belong to `foo`. |
| `BEGIN_PROOF foo` not present                | Property `foo` was never attempted (script didn't reach that line).           |

SAW aborts the script on a failed `llvm_verify`, so the closing
`PROVED` line is the unambiguous success signal — its absence after a
`BEGIN_PROOF` means failure.

## Parsing

The reference parser is
[`scripts/Parse-PropertyLog.ps1`](../scripts/Parse-PropertyLog.ps1).
It scans a SAW stdout/stderr log line-by-line, pairs `BEGIN_PROOF`
markers with their `PROVED` counterparts, and emits one `result.json`
per property under `<output-dir>/properties/<name>/result.json`
following the schema-1 contract in [`result-json.md`](result-json.md).

Properties that have a `BEGIN_PROOF` but no matching `PROVED` are
written with `verdict: "DISPROVED"` and the slice of log lines between
the two markers is preserved as the counterexample evidence.

A minimal grep recipe for ad-hoc inspection:

```bash
# all proven properties
grep -E '^PROVED ' saw.log | awk '{print $2}'

# all attempted properties
grep -E '^BEGIN_PROOF ' saw.log | awk '{print $2}'

# failures = attempted − proven
comm -23 \
  <(grep -E '^BEGIN_PROOF ' saw.log | awk '{print $2}' | sort) \
  <(grep -E '^PROVED '       saw.log | awk '{print $2}' | sort)
```

## Coexistence with the legacy `=== VERIFIED ===` banner

The C++ flow continues to emit the human-readable
`=== Checking: foo == foo_spec (Cryptol) ===` /
`=== VERIFIED: foo == foo_spec ===` banner lines before/after each
proof, and the Rust flow continues to emit a final `print "VERIFIED"`
line.  Those are retained so existing log-readers and the
`verify.ps1` regex match on `VERIFIED` keep working — the new markers
are purely additive.

Producers (the SAW-script emitter) MUST emit the markers in addition
to the legacy lines.  Consumers SHOULD prefer the markers over the
banner, because the markers are stable, per-property, and don't
duplicate the Cryptol-fn name.

## Stability guarantees

- The strings `BEGIN_PROOF ` and `PROVED ` (each with the trailing
  ASCII space) will never change within schema `1`.
- `<name>` will always be a single whitespace-free token.
- Each marker appears on its own line.
- There is exactly one `BEGIN_PROOF` and at most one `PROVED` per
  emitted verification command.

Breaking changes require bumping the result-json schema version and
documenting the migration here.

## Related issues

- `saw-spec-gen-dtb` — introduced this contract.
- `saw-spec-gen-0ft` — multi-property Cryptol bundles (will reuse
  these markers around each emitted `prove_print`).
