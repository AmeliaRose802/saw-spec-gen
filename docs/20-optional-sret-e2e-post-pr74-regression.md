# Optional Sret E2E Still Fails After PR74 Merge

## Status: RESOLVED

Root cause: the LLVM IR parameter parser did not recognise the
`initializes((lo, hi))` attribute that Clang (LLVM 20+) emits on sret
slots. Because `initializes(` was missing from `PAREN_ATTR_PREFIXES`
in `src/parsers/llvm_ir/attrs.rs`, its tokens (`initializes((0,` /
`17))`) were mis-classified as *type fragments* and appended to the
sret param's type string — turning `%"class.std::optional"` into an
unsized opaque type. That defeated the sret byte-size fallback, so no
size was recorded for `std::optional<...>` and the alias could not be
rewritten to `llvm_array N (llvm_int 8)`.

Fix: add `initializes(` to `PAREN_ATTR_PREFIXES` so it is skipped as a
balanced parenthesized attribute group (like `range(`, `align(`, …).
The sret pointee type now resolves to a 17-byte struct, the fallback
records the size, and the alias is lowered before SAW load. The
`optional_sret` VERIFIED / DISPROVED cases in `tests/e2e/cases.psd1`
now reach proof obligations through the default `cpp` runner.

## Summary

`verify-cpp` can still fail on `std::optional<T>` return paths in saw-spec-gen's own `optional_sret` fixtures without any custom runner logic. This is an upstream saw-spec-gen pipeline issue, not a demo_protocol custom-runner issue.

## Exact Command Used

From `C:\Users\ameliapayne\saw-spec-gen`:

```powershell
.\target\release\saw-spec-gen.exe verify-cpp \
  --cpp-file tests\e2e\cases\10-stl-coverage\optional_sret\get_byte_key_verified.cpp \
  --cryptol-spec tests\e2e\cases\10-stl-coverage\optional_sret\get_byte_key_spec.cry \
  --cryptol-fn get_byte_key_spec \
  --function get_byte_key \
  --output tests\e2e\cases\10-stl-coverage\optional_sret\out_manual_verified_repro \
  --cxx-standard c++20
```

## Observed Error

Key output:

```text
note: SAW could not load the -O0 build (empty-struct global load); retrying once at -O1.
...
unsupported type: %"std::optional<proto::ByteKey>"
Details:
Unknown type alias Ident "std::optional<proto::ByteKey>"
RESULT: UNKNOWN
```

Generated SAW script still contains unresolved alias allocation:

```saw
result_ptr <- llvm_alloc (llvm_alias "std::optional<proto::ByteKey>");
```

(Path: `tests/e2e/cases/10-stl-coverage/optional_sret/out_manual_verified_repro/verify.saw`)

## Why This Blocks Verification

This blocks proof obligations for optional-sret cases that should be handled by the merged optional/layout rewrite path. Instead of proving/disproving the Cryptol relation, SAW aborts at type loading with `unknown type alias`.

That means this path still does not "just work" through the default `verify-cpp` flow.

## Required Enhancement

saw-spec-gen should ensure alias-rewrite for complex STL templates applies on the emitted script in the retry path as well (or otherwise guarantee that `std::optional<...>` is lowered to `llvm_array N (llvm_int 8)` before SAW load).

Concretely:

1. Ensure `std::optional<ns::T>` aliases are rewritten for the final emitted `verify.saw` after any `-O0 -> -O1` retry flow.
2. Add an assertion/regression test that the generated script for `optional_sret` contains `llvm_array` and does not contain `llvm_alias "std::optional<...>"` in the final verification artifact.
3. Keep this in default `cpp` runner flow (no custom runner required).

## Local/demo_protocol Follow-Up Completed

A separate local issue in demo_protocol was fixed by adding missing key-store shaping entries in `cpp/saw/saw-spec-gen.toml` for:

- `keyStoreActivateRet`
- `keyStoreProvisionRet`

This removed local type-arity UNKNOWNs so key-store runs now reach proof obligations (DISPROVED), confirming the remaining `optional_sret` alias problem is upstream.
