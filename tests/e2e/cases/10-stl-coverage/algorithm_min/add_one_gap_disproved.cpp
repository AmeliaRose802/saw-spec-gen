/*
DEMO: std::min<uint32_t>(x+1, x+1) — equals x+1 trivially.

Like the algorithm_max case, this exercises a header-only STL
template (`std::min`). The current tool treats functions from
`<algorithm>` as system-header externals and emits an adversarial
havoc spec for them. That over-approximates: the havoc lets the
solver pick ANY return value, so even `std::min(y, y)` is treated
as if it could return any uint32_t, and the equivalence against
`x+1` becomes refutable.

This case is intentionally on the "expected DISPROVED" side of
the suite to document the compositional havoc gap for header-only
STL templates. When the tool learns to symbolic-execute such
trivially-inlinable bodies (or applies a smart override for the
known-pure subset of <algorithm>), rename this file back to
add_one_verified.cpp and flip the expected verdict to VERIFIED.

See tests/e2e/cases/10-stl-coverage/README.md for the full
coverage matrix.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::min(y, y);
}
