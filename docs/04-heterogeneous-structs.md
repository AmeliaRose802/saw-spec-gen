# Heterogeneous struct out-buffers

Mixed-width writable objects are now supported by `--out-buffer-param`.

Use:

```text
--out-buffer-param obj=struct:MyType
--cryptol-fn-out   obj=my_type_post
```

This emits `llvm_alloc (llvm_struct "struct.MyType")`. SAW then allocates one
typed memory cell per LLVM field, honors the struct's natural alignment, and
leaves implicit padding bytes unconstrained.

That matters for layouts such as:

```cpp
struct EnrollmentKey {
    std::uint8_t engaged;
    std::int64_t createdAt;
};
```

`engaged` is byte-granular, `createdAt` is an aligned 64-bit word, and there
are 7 padding bytes between them. A flat byte array rejects the `i64` store;
an `i64` array rejects the byte access. The struct-typed allocation supports
both.

On the Cryptol side, `--cryptol-fn-out` receives the typed fields in order as a
tuple. For the example above, the post-state model can be written against
`([8], [64])` without mentioning padding at all.

See `tests/e2e/cases/09-stateful/enrollment/` for verified and disproved
end-to-end fixtures.
