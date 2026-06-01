/*
DEMO: unique_ptr with wrong value (x+2). DISPROVED.
*/

#include <cstdint>
#include <memory>

uint32_t add_one(uint32_t x) {
    auto p = std::make_unique<uint32_t>(x + 2u);
    return *p;
}
