/*
DEMO: std::accumulate over a 2-element array {x, 1}, starting
from 0 — the sum is `x + 1` for all x where it doesn't overflow.

With the Crucible-safety analyzer (saw_spec_gen-9x5), the tool
recognises that the instantiated `std::accumulate` body (loop
over a contiguous range, arithmetic accumulator) is
Crucible-safe and lets Crucible symbolically execute it. The
`std::array` storage is concrete so the iterator pair resolves
to plain pointer arithmetic.

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to bisect.
*/

#include <cstdint>
#include <numeric>
#include <array>

uint32_t add_one(uint32_t x) {
    std::array<uint32_t, 2> arr{x, 1u};
    return std::accumulate(arr.begin(), arr.end(), 0u);
}
