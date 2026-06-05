# Proof markers (`saw-spec-gen-dtb`)

Sanity check for the `BEGIN_PROOF` / `PROVED` marker contract that
`saw-spec-gen` injects around every emitted `llvm_verify` /
`prove_print` (see [`docs/proof-markers.md`](../../../docs/proof-markers.md)).

The single case in this directory exercises
[`scripts/Parse-PropertyLog.ps1`](../../../scripts/Parse-PropertyLog.ps1)
on a synthetic SAW log:

- Two successful `BEGIN_PROOF` / `PROVED` pairs (`add_one`,
  `double_input`) → two `result.json` files with `verdict=VERIFIED`.
- One opened-but-never-closed `BEGIN_PROOF broken` (no matching
  `PROVED`) → `verdict=DISPROVED` with the surrounding log slice
  preserved as `counterexample[0].value` evidence.

The check script has zero toolchain dependencies — no SAW, no clang,
no rustc — so it runs everywhere the e2e harness does. The marker
*emission* contract (that the SAWScript emitters wrap every
`llvm_verify` with the markers) is covered separately by the Rust
unit tests in
[`src/emit/saw_emit/verify_script_steps_tests.rs`](../../../src/emit/saw_emit/verify_script_steps_tests.rs)
and [`src/gen_verify_rust.rs`](../../../src/gen_verify_rust.rs).
