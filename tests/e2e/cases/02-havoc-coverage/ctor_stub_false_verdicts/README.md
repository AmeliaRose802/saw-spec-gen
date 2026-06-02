# Adversarial holes (FIXED)

Two `add_one` variants originally designed to expose unsound behaviour
in `saw-spec-gen`. Both are now handled correctly and kept as
regression tests — flip either back to its buggy state and CI fails.

| File | Spec direction | Tool result | Real answer |
|------|----------------|-------------|-------------|
| [`add_one_verified.cpp`](add_one_verified.cpp) | should be **VERIFIED** | **VERIFIED** | code is correct |
| [`add_one_disproved.cpp`](add_one_disproved.cpp) | should be **DISPROVED** | **DISPROVED** (real CEX) | code is wrong |

Earlier revisions embedded the historical false verdict in the
filename (`add_one_false_disproved_ctor_stub.cpp` and
`add_one_false_verified_mutable.cpp`); the false-verdict history is
preserved in the section headers below.

Both share [`add_one_spec.cry`](add_one_spec.cry) and run via `verify.ps1`.

## Hole #1 — ctor stubs now initialize data members

Demoed by [`add_one_verified.cpp`](add_one_verified.cpp) (previously
falsely DISPROVED).

The auto-generated `<class>_ctor_override` allocated 8 bytes and wrote
only the vptr, so non-vptr members read as fresh symbolics after
construction — fabricating counterexamples for obviously-correct code.

**Fix:** when a polymorphic class has data members, the ctor override
allocates the full class layout (`llvm_alloc (llvm_alias "class.<Layout>")`)
and writes the vptr + each member's in-class initializer in one
`llvm_struct_value`. The layout name walks the inheritance chain so
derived classes that add no fields reuse the polymorphic base's
layout.

Touched: `src/parsers/clang_ast/` (collect fields, default initializers,
inheritance, `mutable` flags; expose `layout_type_name`/`layout_fields`
on `ClassConstructor`) and `src/emit/saw_emit/` (struct-typed
allocation + `llvm_struct_value` initializer; legacy 8-byte path kept
for empty interface classes; `operator_new` bumped 8 → 256 bytes).

## Hole #2 — `mutable` keyword soundly downgrades `const this`

Demoed by [`add_one_disproved.cpp`](add_one_disproved.cpp) (previously
falsely VERIFIED).

`const` virtual methods always allocated `this` as
`llvm_alloc_readonly` — unsound when the class has a `mutable` field,
because a `const` method can legally write to it.

**Fix:**

1. The `clang_ast` parser tracks classes with any `mutable` data
   member and propagates the flag through inheritance.
2. When building `this`, a `const` method downgrades to
   `Mutability::Mutable` if its class has a `mutable` field.
3. New `emit_this_full_class_havoc` path: allocate `this` as the
   typed alias, set up symbolic pre-state for vptr + each member,
   emit a postcondition that preserves the vptr while writing fresh
   symbolics over each field.

A `const` virtual method on a class with `mutable` data can now
clobber those fields; any equivalence proof depending on field
preservation fails with a real counterexample.

`mutable` is everywhere in production C++ (caches, lazy init, ref
counts, `std::mutex`, `std::once_flag`) — silently unsound on those
patterns would mean tests that should fail spuriously pass.

## Running

```powershell
# Should be VERIFIED — and is.
./verify.ps1 -CppFile tests\e2e\cases\02-havoc-coverage\ctor_stub_false_verdicts\add_one_verified.cpp `
             -CryptolSpec tests\e2e\cases\02-havoc-coverage\ctor_stub_false_verdicts\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one

# Should be DISPROVED (counterexample at `this_bias__post`) — and is.
./verify.ps1 -CppFile tests\e2e\cases\02-havoc-coverage\ctor_stub_false_verdicts\add_one_disproved.cpp `
             -CryptolSpec tests\e2e\cases\02-havoc-coverage\ctor_stub_false_verdicts\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one
```
