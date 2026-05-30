/*
DEMO: std::clamp(x+5, 0u, 5u) — saturates large `x` to 5, so for
x >= 1 the result is at most 5, never `x+1`. Disproved
straightforwardly. Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    return std::clamp<uint32_t>(x + 5u, 0u, 5u);
}
