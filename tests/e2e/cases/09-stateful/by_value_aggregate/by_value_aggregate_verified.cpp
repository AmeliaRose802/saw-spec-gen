// by_value_aggregate — regression for the pass-by-value *aggregate*
// mutability fix.
//
// The MSVC C++ ABI lowers a by-value aggregate that is larger than a
// register (here 16 bytes) into a hidden indirect pointer that the
// *callee* owns and may freely mutate. saw-spec-gen must allocate that
// backing buffer as MUTABLE — a readonly alloc turns the callee's own
// `r.d = 0` store into "Pointer passed to store didn't point to a
// MUTABLE allocation" and drives the whole proof vacuous.
//
// `reset_d` writes to its by-value copy (zeroes `d`) before reading the
// aggregate back. verified : the model returns `a` (the post-write `d`
// is always 0), matching the implementation.

#include <cstdint>

struct Rec {
    std::uint32_t a;
    std::uint32_t b;
    std::uint32_t c;
    std::uint32_t d;
};

// Zero the local copy's `d`, then return a + d (== a). The `r.d = 0`
// store is what requires the hidden by-value param alloc to be MUTABLE.
std::uint32_t reset_d(Rec r) noexcept {
    r.d = 0;
    return r.a + r.d;
}
