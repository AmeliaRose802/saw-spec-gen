/*
DEMO: std::min<uint32_t>(x+1, x+2) — equals x+1 wherever there is
no overflow, but the havoc override emitted for `std::min` is
unconstrained, so the equivalence with `add_one_spec` is refuted
trivially. Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    return std::min(x + 1u, x + 2u);
}
