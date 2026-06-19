# One-pager: verifying stateful methods (the R3 gap)

**Tool:** saw-spec-gen · **Status:** proposal · **Motivating gap:** `KeyStore::provision` / `KeyStore::activate`

## The gap in one sentence

saw-spec-gen today only generates **pure functional-equivalence** specs —
`f(x) == model(x)` — so it cannot express a method whose effect is to **mutate
object state**, which is exactly where this protocol's headline safety invariant
(P1: *Active is irreversible*) actually lives.

## Why this matters here

The whole point of `KeyStore` is the state machine, not a return value:

```
[No Key] --provision--> [Provisional] --activate--> [Active]   (sealed)
```

From `cpp/src/key_store.cpp`, the object's entire state is one member:

```cpp
std::optional<EnrollmentKey> key_;   // guarded by mu_
```

and the two transitions read **and write** it:

```cpp
std::optional<EnrollmentKey> KeyStore::provision(EnrollmentKey newKey) {
    if (key_.has_value() && key_->isActive) return std::nullopt; // P1: never overwrite Active
    if (key_.has_value())                   return std::nullopt; // TOFU: never overwrite Provisional
    newKey.isActive = false;
    key_ = std::move(newKey);
    return key_;
}

ActivationResult KeyStore::activate(const Uuid& keyId) {
    if (!key_.has_value())        return ActivationResult::IoFailure;
    if (key_->keyId != keyId)     return ActivationResult::IoFailure;
    if (key_->isActive)           return ActivationResult::AlreadyActive; // P1
    key_->isActive = true;                                                // the mutation
    return ActivationResult::Success;
}
```

The property we want to machine-check is **relational over the pre/post heap**:

> For all reachable states *s*, `activate` never produces a state where a key
> that *was* `isActive` becomes not-active; and `provision` never overwrites a
> key that *was* `isActive`.

`provisionKey` (the **decision** function, already ✅ proven) captures the
*pure* truth table — given booleans `keyIsActive`, etc., what outcome should
result. But it is given `keyIsActive` as an **input**; it never demonstrates
that the real object's stored `isActive` bit actually obeys the transition. That
last mile — "the stored state evolves as the truth table says" — is the R3 gap.

## What SAW can already do (so this is a generator gap, not a prover gap)

SAW's LLVM frontend natively supports stateful contracts. A hand-written spec
looks like:

```
let activate_spec = do {
    this <- llvm_alloc (llvm_struct "class.sdep::KeyStore");
    // PRE-state: a provisional key is present
    isActive_pre <- llvm_fresh_var "isActive_pre" (llvm_int 8);
    llvm_points_to (llvm_field this "key_.isActive") (llvm_term isActive_pre);
    llvm_precond {{ isActive_pre == 0 }};            // start Provisional
    keyId <- llvm_alloc_readonly ...;

    llvm_execute_func [this, keyId];

    // POST-state: the stored bit is now set, and the return code agrees
    llvm_points_to (llvm_field this "key_.isActive") (llvm_term {{ 1 : [8] }});
    llvm_return (llvm_term {{ `ActivationResult_Success }});
};
```

and the dual *negative* spec asserts that from `isActive_pre == 1` the method
**leaves the bit set** and returns `AlreadyActive`. SAW verifies both against the
real bitcode. **The prover handles this fine** — there is no missing SAW
capability. What is missing is saw-spec-gen *emitting* it.

## What saw-spec-gen would need to add

1. **Detect statefulness.** From the clang AST: a non-`const` method on a class
   with non-static data members, whose body writes a member (or the
   `[[nodiscard]] bool isActive() const` companion exists). Today the generator
   treats `this` as just another opaque pointer arg and produces no heap
   post-conditions.

2. **A pre/post state vocabulary in the spec model.** Let the model author write
   two Cryptol-level views — `model_pre : State -> Args -> bool` (precondition)
   and `model_post : State -> Args -> (State, Ret)` (transition) — and have
   saw-spec-gen wire `this`'s member layout to those `State` fields via
   `llvm_points_to` on both sides of `llvm_execute_func`. This is the
   generalization of the existing `--out-buffer-param` / `--cryptol-fn-out`
   machinery (which already splits a *buffer* into pre/post); here the "buffer"
   is the object's member region.

3. **Member-layout resolution.** Map `KeyStore::key_` (an
   `std::optional<EnrollmentKey>`) to concrete field offsets. The
   `optional<EnrollmentKey>` engaged-flag + payload is the same STL-layout
   problem as R2, so in practice the first cut should target a **plain-struct
   state** (e.g. a `struct { bool present; bool isActive; Uuid id; }`), or a
   `-O1` build where the optional is inlined to byte stores — the same
   workaround already used for `getStatus` / `enforceAccess`.

4. **Generate the dual obligation.** For an irreversibility invariant, emit the
   matched pair automatically: the *forward* transition spec **and** the
   *no-revert* spec from the already-Active precondition. A single
   `--state-invariant isActive:monotone` flag could expand to both.

## Suggested scope (smallest useful step)

A `--stateful` mode that, given:

- the method symbol (`KeyStore::activate`),
- a model file exposing `activate_pre` / `activate_post` over a declared
  `KeyState` record, and
- a `--state-struct class.sdep::KeyStore` layout hint,

emits a SAW script that allocates `this`, constrains the pre-state members from
`activate_pre`, runs the method, and asserts the post-state members and return
value from `activate_post`. Prove it against `key_store_o1.bc` (optional inlined,
per the existing `-O1` workaround). Ship the `KeyStore` P1 pair as the first
fixture.

## Definition of done

- `KeyStore::activate` carries a ✅ **Proven** badge for *both* the
  Provisional→Active transition **and** the Active→Active no-revert case,
  verified against real bitcode — not the pure `provisionKey` truth table alone.
- `KeyStore::provision`'s "never overwrite Active / never overwrite Provisional"
  guards are proven the same way.
- The generator path is general enough that any non-`const` mutating method with
  a declared pre/post model gets a stateful spec instead of being dropped to
  ⚠️ R3.

## Honest caveats

- **Concurrency is out of scope.** The real `KeyStore` is mutex-guarded; SAW
  verifies the *sequential* transition. The "no two threads race the
  provision/activate window" claim (G30) is a separate argument and must not be
  implied by this proof.
- **Optional/STL layout is the practical blocker (R2), not the spec form.** The
  stateful spec is straightforward; getting SAW to see through
  `std::optional`'s engaged flag at `-O0` is the same heap-types wall. The
  `-O1`-inlined-state workaround keeps this tractable for the demo but should be
  documented as a modeling assumption, not hidden.
