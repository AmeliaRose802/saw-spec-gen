/*
DEMO: returning `.second` instead of `.first`. The Cryptol spec
expects `x+1` but this function returns `x`, so SAW should easily
find the counterexample (e.g. x = 0, returns 0, spec says 1).

Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <utility>

uint32_t add_one(uint32_t x) {
    std::pair<uint32_t, uint32_t> p{x + 1u, x};
    return p.second;
}
