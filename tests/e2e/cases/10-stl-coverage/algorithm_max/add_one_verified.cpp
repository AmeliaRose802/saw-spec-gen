/*
DEMO: std::max<uint32_t> over (x+1, x+1) — equals x+1 trivially.

Even though the algebraic answer is clearly `x+1`, the current
tool emits an adversarial havoc override for `std::max` (since
`<algorithm>` is a system header). The havoc allows the solver
to pick ANY return value, so SAW disproves the equivalence
against `add_one_spec`. This is the documented compositional
havoc gap for header-only STL templates.

Expected RESULT: DISPROVED (compositional havoc gap). When the
tool learns to inline trivially-pure STL templates (or ships
smart overrides for the math-only subset of <algorithm>), flip
this case to VERIFIED.

See tests/e2e/cases/10-stl-coverage/README.md.
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::max(y, y);
}
