/*
DEMO: std::string with the WRONG length pushed (x + 2 instead
of x + 1). Even under the strongest possible string model, this
can never match the spec. Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 2u);
    return static_cast<uint32_t>(s.size());
}
