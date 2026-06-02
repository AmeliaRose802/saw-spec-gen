/*
DEMO: std::max<uint32_t> over (x+1, x+1) — equals x+1 trivially.

With the Crucible-safety analyzer (see saw_spec_gen-9x5), the tool
recognises that `std::max<uint32_t>`'s instantiated body is pure
arithmetic + comparisons and lets Crucible symbolically execute it
instead of emitting an adversarial havoc override. The equivalence
against `add_one_spec x = x + 1` is provable on both Linux
(libstdc++) and Windows (MSVC STL).

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to restore the pre-9x5
behaviour (verdict would flip back to DISPROVED via the havoc
override).

See tests/e2e/cases/10-stl-coverage/README.md.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::max(y, y);
}
