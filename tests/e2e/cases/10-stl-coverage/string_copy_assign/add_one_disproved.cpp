#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string src;
    src.resize(x + 1u);
    std::string dst;
    dst = src;
    return static_cast<uint32_t>(dst.size() + 1u);
}
