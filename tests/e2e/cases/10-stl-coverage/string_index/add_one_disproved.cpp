#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 2u);
    char c = s[0];
    (void)c;
    return static_cast<uint32_t>(s.size() - 1u);
}
