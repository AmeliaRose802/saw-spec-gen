#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    s.reserve(x + 128u);
    return static_cast<uint32_t>(s.size() + 2u);
}
