/*
DEMO: returning std::get<1> (the second tuple element) — value
`x`, spec wants `x+1`. Disproved. Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <tuple>

uint32_t add_one(uint32_t x) {
    std::tuple<uint32_t, uint32_t> t{x + 1u, x};
    return std::get<1>(t);
}
