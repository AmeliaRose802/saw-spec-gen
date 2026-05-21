// Control: plain add_one with no exceptions. Matches the Cryptol spec
// directly on every input.

#include <cstdint>

uint32_t add_one(uint32_t x) {
    return x + 1;
}
