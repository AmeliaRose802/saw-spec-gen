# One-pager: uninterpreted primitives (crypto / opaque callees)

**Tool:** saw-spec-gen · **Status:** implemented · **Motivating gap:** `is_valid_signature` (HMAC-SHA256), C++ `isValidSignature` / `hmacSha256`

> **Implemented in [`src/uninterpreted.rs`](../src/uninterpreted.rs).** Both
> declaration surfaces are live: the `@uninterpreted` Cryptol annotation
> (primary) and the `[[uninterpreted]]` section of `saw-spec-gen.toml`. No new
> CLI flag was added. The generator emits an `llvm_unsafe_assume_spec` binding
> for each declaration and appends it to the `llvm_verify` override list on both
> the C++ and Rust paths. Constant-time compare symbols (`ct_eq`,
> `CRYPTO_memcmp`, `ConstantTimeEq`) are recognized and annotated. End-to-end
> coverage lives in `tests/e2e/cases/08-overrides/uninterpreted/`.

## The gap in one sentence

saw-spec-gen's default spec is **pure functional equivalence** — it symbolically
executes the whole callee and asserts `f(x) == model(x)` — but some callees
(SHA-256, HMAC, AEAD) are infeasible to execute symbolically and must instead be
treated as **uninterpreted functions** with a Cryptol contract, so the prover
reasons about *callers* compositionally rather than unfolding the primitive.

## Motivating code

```rust
pub fn is_valid_signature(key: &[u8;32], request: &DeviceRequest, sig: &[u8]) -> bool {
    let payload = canonicalize_payload(request);
    let mut mac = HmacSha256::new_from_slice(key)...;
    mac.update(payload.as_bytes());
    let expected = mac.finalize().into_bytes();
    expected.ct_eq(sig).into()           // constant-time compare
}
```

`HmacSha256::finalize` expands to the full SHA-256 compression function — SAW
cannot close that in finite time. But we don't *want* to re-verify SHA-256; we
want to assume `hmacSha256` behaves like its Cryptol model and prove the
*caller's* logic (payload construction, length guard, constant-time compare).

## This is a generator gap, not a prover gap

SAW already supports exactly this via `llvm_unsafe_assume_spec` /
`mir_unsafe_assume_spec`. An assumed spec binds a symbol to a Cryptol contract
and **applies globally** — it does not even need to appear in an override list.
The only thing missing is a way for saw-spec-gen to *emit* one from a
declaration. Target generated output:

```
hmac_spec <- llvm_unsafe_assume_spec mod "<hmac_symbol>" (do {
    key <- llvm_fresh_var "key" (llvm_array 32 (llvm_int 8));
    (msg, msg_ptr, n) <- ptr_len_arg "msg";
    llvm_execute_func [key_ptr, msg_ptr, n];
    llvm_return (llvm_term {{ hmacSha256 key msg }});
});
```

## Declaration surface — no CLI flag (per request)

Two reproducible, version-controllable surfaces; **(a) is primary** because it
lives next to the model:

**(a) Cryptol-spec annotation.** A marker doc-comment on the Cryptol declaration
of the primitive:

```cryptol
/** @uninterpreted */                 // resolve impl symbol by name/mangling
hmacSha256 : [32][8] -> [n][8] -> [32][8]

/** @uninterpreted symbol="?HmacSha256@@..." */   // explicit symbol override
```

**(b) `saw-spec-gen.toml` (project config, already supported).** Extend
`ProjectConfig` (`src/project_config.rs`) with a new section, both global and
per-function-friendly:

```toml
[[uninterpreted]]
cryptol_fn = "hmacSha256"     # Cryptol model used as the assumed contract
symbol     = "..."            # optional; omit to resolve by name/mangling
```

Either way the value is in git, reviewable in a diff, and self-documenting —
unlike a flag buried in a `run.ps1` array.

## Constant-time compare

`subtle::ct_eq` / `CRYPTO_memcmp` should be recognized as **equality** and
lowered to a plain `==` postcondition (or shipped as a built-in assumed spec in
the override registry alongside the existing STL families), so the length-guard
+ comparison branch verifies without a per-project declaration.

## Scope / non-goals

* In scope: emitting `*_unsafe_assume_spec` from `@uninterpreted` declarations;
  threading the resulting handle into the caller verification; a small built-in
  set of recognized crypto-compare symbols.
* Out of scope: proving the primitives themselves (that is a separate, one-time
  Cryptol↔reference effort), and any new CLI flag.

## Acceptance

`is_valid_signature` verifies (DISPROVED on a deliberately wrong payload model;
PROVEN on the correct one) with `hmacSha256` declared `@uninterpreted` and **zero
CLI flags** added to the demo_protocol `rust/saw` driver.
