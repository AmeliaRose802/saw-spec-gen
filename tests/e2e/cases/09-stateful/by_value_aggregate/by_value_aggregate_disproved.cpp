// by_value_aggregate (disproved) — same by-value-aggregate mutability
// path as the verified case, but with a buggy body.
//
// This variant zeroes the *wrong* field (`c` instead of `d`), then
// returns `a + d`. The model `reset_d_ret` says the result is `a`, so
// for any non-zero `d` the implementation returns `a + d != a` — a
// genuine counterexample. The `r.c = 0` store still exercises the
// MUTABLE by-value backing allocation (a readonly alloc would crash
// vacuously *before* the solver ever finds the real bug).

#include <cstdint>

struct Rec {
    std::uint32_t a;
    std::uint32_t b;
    std::uint32_t c;
    std::uint32_t d;
};

// Bug: zeroes `c` (not `d`) and returns a + d. DISPROVES against the
// `reset_d_ret` model (which returns `a`) whenever `d != 0`.
std::uint32_t reset_d(Rec r) noexcept {
    r.c = 0;
    return r.a + r.d;
}
