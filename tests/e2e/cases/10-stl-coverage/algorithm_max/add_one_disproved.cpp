/*
DEMO: std::max<uint32_t>(x+1, x+2) — equals x+2 for all x where x+2
doesn't overflow, and equals 1 only when x = UINT32_MAX. Either way
this is not the spec (x+1) on every input, so SAW finds a counterexample.

Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    return std::max(x + 1u, x + 2u);
}
