/*
DEMO: BUGGY 16-bit byte swap — returns x unchanged.

SAW disproves with any x whose two bytes differ (e.g. x = 0x00FF:
impl returns 0x00FF, spec says 0xFF00).

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint16_t swap_bytes_u16(uint16_t x) {
    return x;   // BUG: missing the shift+or composition.
}
