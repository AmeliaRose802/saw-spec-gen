/*
DEMO: BUGGY bool-or-to-u32 — uses AND instead of OR.

SAW disproves with a=true, b=false (impl returns 0, spec says 1).

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint32_t bool_or_to_u32(bool a, bool b) {
    return (a && b) ? 1u : 0u;   // BUG: was a || b.
}
