/*
DEMO: std::accumulate over a 2-element array {x, 1}, starting
from 0 — the sum is `x + 1` for all x where it doesn't overflow.

`std::accumulate` lives in `<numeric>`, so the same compositional
havoc gap applies as for `<algorithm>`: the auto-spec lets the
return value be anything, and the equivalence is refuted.

Expected RESULT: DISPROVED (compositional havoc gap on
std::accumulate).
*/

#include <cstdint>
#include <numeric>
#include <array>

uint32_t add_one(uint32_t x) {
    std::array<uint32_t, 2> arr{x, 1u};
    return std::accumulate(arr.begin(), arr.end(), 0u);
}
