/*
DEMO: function that takes a `void*` parameter (without dereferencing it)
and returns whether the pointer compares equal to a second `void*`.

Coverage: target function exercising the `void*` parameter lowering path
that bug-fix #11 patched (`pointee_saw_type` rewrites the `// void`
sentinel comment to `llvm_int 8`). Without that fix, the generated SAW
spec would be syntactically broken (line comment swallowing the
`llvm_alloc` expression).

The function never dereferences either pointer, so the choice of pointee
size doesn't affect correctness.
*/

#include <cstdint>

uint32_t void_ptr_eq(const void* a, const void* b) {
    return a == b ? 1u : 0u;
}
