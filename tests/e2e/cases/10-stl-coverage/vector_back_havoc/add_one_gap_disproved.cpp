/*
DEMO: std::vector::push_back + back() — the canonical "smart
container" workflow. Allocates a heap buffer, copies a value in,
then reads it back.

This is intentionally on the gap side: SAW's MIR/LLVM-level
allocator model does not unify with `std::vector`'s internal
allocator, and the auto-generated havoc overrides for
`emplace_back`, `_Emplace_one_at_back`, and the allocator
hierarchy can't model "you wrote x+1 into the buffer; you read
x+1 back" as a coupled pair of state edits. The havoc lets
`back()` return any value, so the equivalence is refuted.

Expected RESULT: DISPROVED (vector compositional gap). When the
tool ships a vector model (or learns to inline single-element
push_back + back() patterns), rename to add_one_verified.cpp and
flip to VERIFIED.
*/

#include <cstdint>
#include <vector>

uint32_t add_one(uint32_t x) {
    std::vector<uint32_t> v;
    v.push_back(x + 1u);
    return v.back();
}
