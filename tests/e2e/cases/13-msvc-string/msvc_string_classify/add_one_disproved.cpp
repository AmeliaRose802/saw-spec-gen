/*
Regression test — DISPROVED variant.

Same as add_one_verified.cpp but resizes to x+2 instead of x+1, so
the implementation can never match `add_one_spec x = x + 1`.

RESULT: DISPROVED.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(static_cast<std::size_t>(x) + 2u);
    return static_cast<uint32_t>(s.size());
}
