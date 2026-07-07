# One-pager: verifying stateful methods (the R3 gap)

**Tool:** saw-spec-gen · **Status:** supported (no new flag) · **Motivating gap:** `KeyStore::provision` / `KeyStore::activate`

## The gap in one sentence

saw-spec-gen's *default* spec is **pure functional-equivalence** —
`f(x) == model(x)` — so by default it cannot express a method whose effect is to
**mutate object state**, which is exactly where this protocol's headline safety
invariant (P1: *Active is irreversible*) actually lives.

## The fix: model the object as a writable buffer (existing flags)

There is **no dedicated flag** for this. A stateful method's `this`/object
pointer is just a writable region, and saw-spec-gen already knows how to express
the pre/post state of a writable region — that is precisely what
`--out-buffer-param` + `--cryptol-fn-out` do for ordinary output buffers. Point
them at the object:

```
--out-buffer-param ks=1               # model `ks` as a 1-byte writable buffer
--cryptol-fn-out   ks=key_store_post  # pin the whole-object post-state
```

The generated spec allocates `ks`, binds a fresh `ks_pre` to its pre-state,
runs the method, then asserts `llvm_points_to ks_ptr (llvm_term {{ key_store_post ks_pre }})`.
That `llvm_points_to` on the post-state side is the stateful contract.

## Why this matters here

The whole point of `KeyStore` is the state machine, not a return value:

```
[No Key] --provision--> [Provisional] --activate--> [Active]   (sealed)
```

The property we want to machine-check is **relational over the pre/post heap**:

> For all reachable states *s*, `activate` never produces a state where a key
> that *was* `isActive` becomes not-active; and `provision` never overwrites a
> key that *was* `isActive`.

## What SAW can already do (so this is a generator gap, not a prover gap)

SAW's LLVM frontend natively supports stateful contracts. A hand-written spec
looks like:

```
let activate_spec = do {
    ks_ptr <- llvm_alloc (llvm_array 1 (llvm_int 8));
    ks_pre <- llvm_fresh_var "ks_pre" (llvm_array 1 (llvm_int 8));
    llvm_points_to ks_ptr (llvm_term ks_pre);

    llvm_execute_func [ks_ptr];

    // POST-state: the stored bit is now set (whole-object model)
    llvm_points_to ks_ptr (llvm_term {{ key_store_post ks_pre }});
    llvm_return (llvm_term {{ key_store_ret ks_pre }});
};
```

**The prover handles this fine** — there is no missing SAW capability, and now no
missing *generator* capability either: `--out-buffer-param` + `--cryptol-fn-out`
emit exactly this.

## Expressing field-level intent in the Cryptol model

Because the post-state is one whole-object Cryptol value, every common per-field
intent is just a value-level expression — no special syntax:

| Intent                       | Cryptol post-state (for `pre`)                          |
|------------------------------|---------------------------------------------------------|
| Set a byte to a constant     | `[1]` (1-byte object)                                   |
| Keep a field unchanged       | `pre @ i` for the kept bytes (e.g. `[1] # drop`{1} pre`) |
| Relational / byte-wise       | `[ byte ^ 0xAB | byte <- pre ]`                          |
| Multi-field                  | concatenate per-field byte groups                       |
| Wide scalar field (`uint32`) | `pre + 1` over `[32]` (shape `c=i32`)                    |

The `tests/e2e/cases/09-stateful/**` fixtures demonstrate each:
`key_store` (single-byte latch), `block` (a `uint8[4]` buffer XOR-ed
byte-by-byte), `session` (set one field, keep the tag bytes), and `counter`
(a wide `uint32` field incremented as one `i32` store).

## Object shape: byte buffers and wide typed fields

The SHAPE in `--out-buffer-param NAME=SHAPE` chooses how the object pointer is
allocated, so the allocation matches how the compiled body accesses memory:

| SHAPE   | SAW allocation               | Use for                                   |
|---------|------------------------------|-------------------------------------------|
| `N`     | `llvm_array N (llvm_int 8)`  | byte-granular fields (`uint8`, `uint8[]`) |
| `iW`    | `llvm_int W`                 | a single wide scalar field (e.g. `uint32` → `i32`) |
| `NxiW`  | `llvm_array N (llvm_int W)`  | a homogeneous array of wide fields (`uint32[4]` → `4xi32`) |
| `auto`  | inferred pointee type        | keep the front-end's inferred layout      |

A byte array satisfies byte-granular access, but **not** a wide typed field:
a bare `uint32` member compiles to a single `i32` load/store, which SAW's
memory model rejects against an `i8`-array allocation (`Error during memory
load`). Declaring the shape as `iW` allocates `llvm_int W`, so the wide access
type-checks and the Cryptol model operates on a `[W]` word — see the `counter`
fixture (`--out-buffer-param c=i32`, `counter_inc_post pre = pre + 1`).

For a **heterogeneous** struct (mixed widths, e.g. `uint8` + `int64_t`), use a
struct-typed out-buffer instead of flattening the object:

```text
--out-buffer-param obj=struct:MyType
--cryptol-fn-out   obj=my_type_post
```

This emits `llvm_alloc (llvm_struct "struct.MyType")`, so SAW uses the LLVM
field layout from the loaded bitcode: typed cells per field, natural alignment,
and implicit padding bytes left unconstrained. The paired `--cryptol-fn-out`
model sees the typed fields as a Cryptol tuple in field order, which is enough
to express mixed-width whole-object post-states without pinning padding.

## When a partial, per-field assertion would help

Whole-object `llvm_points_to` pins **every** byte, so the model must account for
the entire region. The one scenario it can't express is "constrain field X, leave
the rest of the object free" — useful only when the object's full byte layout is
genuinely un-modellable (e.g. an `std::optional`/STL member at `-O0` whose padding
and engaged-flag bytes you can't pin). That partial-assertion case is intentionally
**not** supported today (YAGNI): the `-O1`-inlined-state workaround keeps the real
targets modellable as plain byte regions, so the whole-object form suffices.

## Definition of done

- `KeyStore::activate` carries a ✅ **Proven** badge for *both* the
  Provisional→Active transition **and** the Active→Active no-revert case,
  verified against real bitcode — not the pure `provisionKey` truth table alone.
- `KeyStore::provision`'s "never overwrite Active / never overwrite Provisional"
  guards are proven the same way.
- Any non-`const` mutating method with a declared whole-object pre/post model
  gets a stateful spec via `--out-buffer-param` + `--cryptol-fn-out` instead of
  being dropped to ⚠️ R3.

## Honest caveats

- **Concurrency is out of scope.** The real `KeyStore` is mutex-guarded; SAW
  verifies the *sequential* transition. The "no two threads race the
  provision/activate window" claim (G30) is a separate argument and must not be
  implied by this proof.
- **Optional/STL layout may still need care at `-O0`.** Struct-typed
  out-buffers solve the mixed-width heap-types problem, but large STL wrappers
  can still be awkward when the compiled body touches compiler-specific helper
  state. The `-O1`-inlined-state workaround remains useful for those cases and
  should be documented as a modeling assumption, not hidden.
