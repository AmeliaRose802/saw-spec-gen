#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    const char* p = s.c_str();
    (void)p;
    return static_cast<uint32_t>(s.size() + 1u);
}
