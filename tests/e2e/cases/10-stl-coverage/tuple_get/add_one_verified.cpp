/*
DEMO: std::tuple<uint32_t> + std::get<0>. Like pair_first but
with std::tuple, which exercises clang's variadic template
machinery. With the Crucible-safety analyzer (saw_spec_gen-9x5),
both the tuple constructor and `std::get<0>`'s body resolve to
plain GEP + load arithmetic on the stack-allocated tuple, and
Crucible executes them directly.

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to bisect.
*/

#include <cstdint>
#include <tuple>

uint32_t add_one(uint32_t x) {
    std::tuple<uint32_t, uint32_t> t{x + 1u, x};
    return std::get<0>(t);
}
