# Adversarial holes (FIXED)

Two `add_one` variants originally designed to expose unsound or imprecise
behavior in `saw-spec-gen`. Both are now handled correctly. They remain
here as regression tests — flip either one back to the buggy state and a
CI run should immediately notice.

| File | Spec direction | Tool result | Real answer | Status |
|------|----------------|-------------|-------------|--------|
| [add_one_false_disproved_ctor_stub.cpp](add_one_false_disproved_ctor_stub.cpp) | should be **VERIFIED** | reports **VERIFIED** | code is correct | fixed |
| [add_one_false_verified_mutable.cpp](add_one_false_verified_mutable.cpp) | should be **DISPROVED** | reports **DISPROVED** (real CEX) | code is wrong | fixed |

Both share [add_one_spec.cry](add_one_spec.cry) and run through `verify.ps1`.

## Hole #1 — fixed: ctor stubs now initialize data members

Demonstrated by [add_one_false_disproved_ctor_stub.cpp](add_one_false_disproved_ctor_stub.cpp).

**Before:** the auto-generated `<class>_ctor_override` allocated only 8
bytes (`llvm_alloc_aligned 8 (llvm_int 64)`) and wrote only the vptr.
Any non-vptr member was read as a fresh symbolic value after
construction, fabricating counterexamples for obviously-correct code.

**Fix:** when a polymorphic class has data members, the ctor override
now allocates the full class layout via
`llvm_alloc (llvm_alias "class.<LayoutName>")` and writes the vptr +
each member's in-class initializer in one `llvm_struct_value`. The
layout type name is computed by walking the inheritance chain: if a
derived class adds no fields, clang reuses the polymorphic base's
layout, so the override uses the base's name; otherwise it uses the
derived class's own name.

Files touched:
- [`src/clang_ast.rs`](../../../src/clang_ast.rs): collect per-class
  field lists, default initializers, inheritance, and `mutable` flags;
  expose `layout_type_name` and `layout_fields` on `ClassConstructor`.
- [`src/saw_emit.rs`](../../../src/saw_emit.rs): emit a struct-typed
  allocation + `llvm_struct_value` initializer for ctors with data
  members; keep the legacy 8-byte path for empty interface classes.
  Also bumped `operator_new`'s auto-spec from 8 to 256 bytes so that
  ctor overrides for larger classes can match.

## Hole #2 — fixed: `mutable` keyword soundly downgrades `const this`

Demonstrated by [add_one_false_verified_mutable.cpp](add_one_false_verified_mutable.cpp).

**Before:** `const` virtual methods always allocated `this` as
`llvm_alloc_readonly`, modeling "this is preserved" — unsound when the
class has a `mutable` field, because a `const` method can legally
write to it.

**Fix:**
1. The `clang_ast` parser now tracks classes that contain any
   `mutable`-qualified data member and propagates that flag through
   inheritance.
2. When constructing the `this` param in `parse_function_decl`, a
   `const` method downgrades to `Mutability::Mutable` if its class
   has any `mutable` field.
3. The havoc-spec generator gets a new `emit_this_full_class_havoc`
   path: when `this` is mutable and the class has known data fields,
   allocate `this` as `llvm_alias "class.<Layout>"`, set up a symbolic
   pre-state for the vptr + each member, and emit a postcondition that
   preserves the vptr while writing fresh symbolics over each field.

A `const` virtual method on a class with `mutable` data is now allowed
to clobber those fields, so any equivalence proof that depends on the
field being preserved will fail with a real counterexample.

**Why this matters:** `mutable` shows up everywhere in production C++
(caches, lazy initialization, ref counts, `std::mutex`,
`std::once_flag`). The previous behavior was silently unsound on those
patterns; tests that should have failed could have spuriously passed.

## Running the examples

```powershell
# Should be VERIFIED — and is.
./verify.ps1 -CppFile end-to-end-test\vtable_havoc_spec_demos\adversarial_holes\add_one_false_disproved_ctor_stub.cpp `
             -CryptolSpec end-to-end-test\vtable_havoc_spec_demos\adversarial_holes\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one

# Should be DISPROVED (with a counterexample at `this_bias__post`) — and is.
./verify.ps1 -CppFile end-to-end-test\vtable_havoc_spec_demos\adversarial_holes\add_one_false_verified_mutable.cpp `
             -CryptolSpec end-to-end-test\vtable_havoc_spec_demos\adversarial_holes\add_one_spec.cry `
             -CryptolFn add_one_spec -Function add_one
```
