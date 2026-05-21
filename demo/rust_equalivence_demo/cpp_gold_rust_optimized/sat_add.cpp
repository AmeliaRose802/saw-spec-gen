#include <cstdint>
#include <climits>

// Saturating add for unsigned 32-bit integers.
//
// If `a + b` would overflow past UINT32_MAX, return UINT32_MAX instead
// of letting it wrap around to a small number.
//
// GOLD REFERENCE: written the obvious, step-by-step way.

uint32_t sat_add(uint32_t a, uint32_t b) {
    uint32_t sum = a + b;
    if (sum < a) {
        // overflow: the wrapped result is smaller than what we started with
        return UINT32_MAX;
    }
    return sum;
}
