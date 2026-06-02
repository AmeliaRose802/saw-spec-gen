/*
DEMO: std::clamp<uint32_t>(x+1, x+1, x+1) — degenerate clamp
where low == high == value, so the result must be x+1 by clamp's
spec.

With the Crucible-safety analyzer (saw_spec_gen-9x5), the tool
recognises `std::clamp<uint32_t>`'s instantiated body as pure
arithmetic + comparisons and lets Crucible symbolically execute
it. Equivalence against `add_one_spec` proves on both Linux and
Windows.

Requires C++17 for `std::clamp`. The runner forwards
`CxxStandard = 'c++17'` from cases.psd1.

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to bisect.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::clamp<uint32_t>(y, y, y);
}
