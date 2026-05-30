/*
DEMO: BUGGY max of two i16 values — returns the *smaller* instead.

SAW disproves with a=1, b=0 (impl returns 0, spec says 1).

Expected verdict: DISPROVED.
*/

#include <cstdint>

int16_t max_i16(int16_t a, int16_t b) {
    return a < b ? a : b;   // BUG: comparison inverted.
}
