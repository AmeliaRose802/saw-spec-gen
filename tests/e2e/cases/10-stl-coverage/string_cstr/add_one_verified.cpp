/*
DEMO: std::string::c_str() leaves size unchanged.

c_str() returns a pointer to the null-terminated character data.
In libstdc++ it is semantically identical to data() for
std::__cxx11::basic_string. Our override (BasicStringCStr) models
it identically to data(): returns a fresh symbolic byte pointer
with no pre-state constraint, so it composes freely with resize().

The c_str() return value is discarded here; we only care that
calling it does not perturb s.size().
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    const char* p = s.c_str();
    (void)p;
    return static_cast<uint32_t>(s.size());
}
