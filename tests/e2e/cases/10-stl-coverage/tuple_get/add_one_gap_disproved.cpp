/*
DEMO: std::tuple<uint32_t> + std::get<0>. Like pair_first but
with std::tuple, which exercises clang's variadic template
machinery. `std::get` is a template function from `<tuple>`, so
the system-header havoc rule applies and the override fires.

Expected RESULT: DISPROVED (compositional havoc gap on std::get).
*/

#include <cstdint>
#include <tuple>

uint32_t add_one(uint32_t x) {
    std::tuple<uint32_t, uint32_t> t{x + 1u, x};
    return std::get<0>(t);
}
