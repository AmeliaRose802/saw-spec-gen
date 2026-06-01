/*
DEMO: vector with WRONG value pushed (x+2 instead of x+1). Even
under the strongest possible vector model, this can never match
the spec. Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <vector>

uint32_t add_one(uint32_t x) {
    std::vector<uint32_t> v;
    v.push_back(x + 2u);
    return v.back();
}
