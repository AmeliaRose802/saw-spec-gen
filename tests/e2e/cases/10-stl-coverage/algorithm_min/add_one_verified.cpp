/*
DEMO: std::min<uint32_t>(x+1, x+1) — equals x+1 trivially.

With the Crucible-safety analyzer (saw_spec_gen-9x5), the tool
recognises `std::min<uint32_t>`'s instantiated body as pure
arithmetic + comparisons and lets Crucible symbolically execute
it. The equivalence against `add_one_spec x = x + 1` proves on
both Linux (libstdc++) and Windows (MSVC STL).

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to restore the pre-9x5
behaviour (verdict would flip back to DISPROVED via the
adversarial-override fallback).

See tests/e2e/cases/10-stl-coverage/README.md for the full
coverage matrix.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::min(y, y);
}
