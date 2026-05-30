# saw-spec-gen bug: unsized pointer parameters silently allocate 1 byte (no `_In_reads_(<param>)` support, no loud TODO)

**Date:** 2026-05-30
**saw-spec-gen commit:** `2071230` (branch `feat-enum-type-constraints`)
**Severity:** P2 — auto-generated `verify.saw` is silently wrong for any function whose pointer parameter has a length conveyed by a sibling parameter
**Reproducer in this repo:**
- C++ source: [cpp/include/sdep/canonical_lp.hpp](cpp/include/sdep/canonical_lp.hpp#L37-L58) (`canonicalize_lp`)
- Auto-generated spec: [cpp/saw/out_canonicalize_lp/verify.saw](cpp/saw/out_canonicalize_lp/verify.saw)
- SAW log: [cpp/saw/out_canonicalize_lp/saw_run.log](cpp/saw/out_canonicalize_lp/saw_run.log)
- saw-spec-gen SAL parser: `src/parsers/clang_ast/sal.rs`
- saw-spec-gen emitter: `src/emit/saw_emit/havoc_params.rs`

---

## Summary

Two related defects:

1. **SAL parameter-reference annotations are dropped.** The SAL parser only
   accepts a decimal literal inside `_In_reads_(...)` / `_Out_writes_(...)`.
   The much more common SAL form
   `_In_reads_(<otherParameterName>)` — used everywhere in real Windows
   headers — silently parses as **no annotation**, even though Clang
   exposes both forms identically as `AnnotateAttr`.

2. **The fallback for unsized pointers is silent.** When a pointer
   parameter has no size annotation, the emitter allocates a 1-byte
   buffer and adds the note `// NOTE: x may be null -- spec assumes
   non-null`. That note doesn't tell the user the buffer length is
   guessed, doesn't name the likely length-companion parameter, and
   doesn't tell the user the resulting spec almost certainly won't
   type-check against any non-trivial Cryptol shim. For a
   length-prefixed canonicalization function like `canonicalize_lp` the
   resulting spec is **unrecoverably broken** unless the user happens
   to be reading the emitted file character-by-character.

The user remembers a clearer "TODO[saw-spec-gen]: fill in buffer
length by hand" comment that the current codebase doesn't emit. The
only `// TODO:` emit left in the verify-script generator is for
compositional postconditions
(`src/emit/saw_emit/verify_script_steps.rs:336`); unsized pointer
parameters no longer get one.

---

## Repro — defect #1 (SAL param ref dropped)

### C++ under test

```cpp
// cpp/include/sdep/canonical_lp.hpp
[[nodiscard]] inline std::size_t
canonicalize_lp(std::uint8_t* out,
                const std::uint8_t* m, std::uint8_t nm,
                const std::uint8_t* b, std::uint8_t nb) noexcept;
```

### What the user wants to write

```cpp
[[nodiscard]] inline std::size_t
canonicalize_lp(_Out_writes_(2 + nm + nb) std::uint8_t* out,
                _In_reads_(nm)            const std::uint8_t* m,
                std::uint8_t               nm,
                _In_reads_(nb)            const std::uint8_t* b,
                std::uint8_t               nb) noexcept;
```

This is the standard SAL form Microsoft uses in `windows.h`,
`bcrypt.h`, etc. — the size is identified by **the name of another
parameter**.

### What saw-spec-gen does today

`src/parsers/clang_ast/sal.rs::classify`:

```rust
let inner_count = |prefix: &str| -> Option<usize> {
    value.strip_prefix(prefix)?
         .strip_suffix(')')?
         .parse::<usize>()      // <-- only accepts decimal literal
         .ok()
};
if let Some(n) = inner_count("_In_reads_(") { return Some(Annotation::InReads(n)); }
```

So `"_In_reads_(nm)"` returns `None`, no annotation is attached, and
the emitter falls into the "unsized pointer" path.

### Expected behaviour

Either:

(a) Add an `Annotation::InReadsParam(String)` /
    `Annotation::OutWritesParam(String)` variant, store the
    parameter-name string when the inner text isn't a decimal literal,
    and have `havoc_params.rs::emit_one_param_setup_and_postcond`
    resolve the name at emit time to "allocate `llvm_array N (llvm_int 8)`
    where `N` is a fresh symbolic upper bound, plus a SAW
    `llvm_precond {{ <named_param_value> <= N }}`".

(b) At minimum, emit a clearly-labelled TODO when the inner text fails
    to parse as `usize`, instead of silently dropping the annotation:

    ```
    // TODO[saw-spec-gen]: _In_reads_(nm) referenced parameter `nm` —
    // parameter-reference SAL is not yet supported. Length annotation
    // was dropped; the buffer below is sized as a single scalar.
    ```

### Test case for the e2e suite

`tests/e2e/cases/<new>/sal_param_ref.cpp`:

```cpp
#include <cstdint>
#include <sal.h>   // or the project's sal_compat.hpp shim

extern "C" std::size_t
copy_n(_Out_writes_(n) std::uint8_t* dst,
       _In_reads_(n)   const std::uint8_t* src,
       std::uint8_t    n);
```

Assertion: the generated `verify.saw` must allocate `dst`/`src` as
arrays whose length is tied to `n` (either symbolic with an
`llvm_precond` bound, or at a chosen finite `MAX_N`).

---

## Repro — defect #2 (silent 1-byte fallback)

### Current emit for `canonicalize_lp`

```saw
let canonicalize_lp_equiv_spec = do {
    out_ptr <- llvm_alloc (llvm_int 8);
    out <- llvm_fresh_var "out" (llvm_int 8);
    llvm_points_to out_ptr (llvm_term out);
    // NOTE: out may be null -- spec assumes non-null
    m_ptr <- llvm_alloc_readonly (llvm_int 8);
    m <- llvm_fresh_var "m" (llvm_int 8);
    llvm_points_to m_ptr (llvm_term m);
    // NOTE: m may be null -- spec assumes non-null
    nm <- llvm_fresh_var "nm" (llvm_int 8);
    b_ptr <- llvm_alloc_readonly (llvm_int 8);
    b <- llvm_fresh_var "b" (llvm_int 8);
    llvm_points_to b_ptr (llvm_term b);
    // NOTE: b may be null -- spec assumes non-null
    nb <- llvm_fresh_var "nb" (llvm_int 8);

    llvm_execute_func [out_ptr, m_ptr, llvm_term nm, b_ptr, llvm_term nb];

    llvm_return (llvm_term {{ canonicalize_lp_post out m nm b nb }});
};
```

This is silently wrong on at least three axes:

- `out`, `m`, `b` are *length-prefixed buffers*, not single bytes.
- The "NOTE: x may be null" wording is misleading — the spec doesn't
  *assume* non-null, it just allocates one byte. The user can't tell
  whether the 1-byte allocation is intentional ("the function reads
  exactly one byte") or a fallback ("we have no idea how big this is").
- The function's name (`canonicalize_lp` = "length-prefixed") and the
  presence of obvious length-companion parameters (`nm`, `nb`) are
  strong heuristic signals that 1 byte is wrong.

### Expected emit

When a pointer parameter has no size annotation **and** the same
function has any integer parameter whose name resembles a typical
length identifier (`n`, `len`, `count`, `size`, `<ptrname>_len`,
`n<ptrname>`, etc.), the emit should be:

```saw
// TODO[saw-spec-gen]: pointer parameter `m` has no size annotation.
// Heuristic guess: the integer parameter `nm` looks like its length
// (matching `n` + ptr-name prefix). The auto-spec below allocates a
// single byte, which is almost certainly wrong for a length-prefixed
// buffer. Fix by either:
//   1. Adding _In_reads_(nm) to the C++ declaration, OR
//   2. Editing this file to allocate (llvm_array N (llvm_int 8))
//      with a SAW precondition `nm <= N`.
m_ptr <- llvm_alloc_readonly (llvm_int 8);   // <-- size guess: 1 byte
```

When there is no obvious length companion, the comment should still
say "auto-spec guessed 1 byte; replace if this is a buffer" rather
than the current "may be null" framing.

---

## Suggested fix locations

| Defect | File | Change sketch |
|---|---|---|
| #1 SAL paramref | `src/constraints/types.rs` | Add `InReadsParam(String)`, `OutWritesParam(String)` variants to `Annotation`. |
| #1 SAL paramref | `src/parsers/clang_ast/sal.rs` | In `classify`, if `parse::<usize>()` fails, fall through to a paramref capture: strip the parens, return the inner string as `Annotation::InReadsParam(...)`. |
| #1 SAL paramref | `src/emit/saw_emit/havoc_params.rs` | In the buffer-size resolution that today only matches `InReads(n)` / `OutWrites(n)`, add a paramref branch that emits `llvm_array MAX (llvm_int 8)` plus `llvm_precond {{ <named_param> <= MAX }}`. Choose `MAX` as a SAW-config knob (defaulting to e.g. 16, with a TODO marker). |
| #2 loud TODO | `src/emit/saw_emit/havoc_params.rs` | When emitting an unsized pointer, replace `// NOTE: x may be null -- spec assumes non-null` with the named-companion-aware `// TODO[saw-spec-gen]: ...` comment described above. |
| #2 loud TODO | (regression test) | `tests/e2e/cases/<new>/unsized_pointer_warning.cpp` — assert the emitted file contains `TODO[saw-spec-gen]` for an unsized pointer. |

---

## Workaround in the affected repo

For `canonicalize_lp` specifically we plan to ship a hand-rolled
`cpp/saw/custom/canonicalize_lp.saw` that binds a fixed `MAX_LEN=4`
and proves the C++ binary matches `canonicalize_lp_post` byte-for-byte
at that bound — see the comment in
[cpp/include/sdep/canonical_lp.hpp](cpp/include/sdep/canonical_lp.hpp#L12-L15).
The Cryptol-level injectivity proof at all `nm,nb ≤ MAX_LEN` is
already in `cpp/saw/SDEP_cpp.cry` and passing (Layer 3, property `P24`
analogue), so the only missing link is the SAW C++-to-Cryptol byte
equivalence — and that link is exactly what the auto-spec is supposed
to give us. We are explicitly *not* committing the hand-rolled file
yet, so this auto-spec stays in-tree as a clean reproducer for the
bug.
