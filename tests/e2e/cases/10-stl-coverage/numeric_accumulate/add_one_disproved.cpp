/*
DEMO: accumulate over {x, 2} — sum is x+2, not x+1. DISPROVED.
*/

#include <cstdint>
#include <numeric>
#include <array>

uint32_t add_one(uint32_t x) {
    std::array<uint32_t, 2> arr{x, 2u};
    return std::accumulate(arr.begin(), arr.end(), 0u);
}
