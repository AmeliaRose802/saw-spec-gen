/*
DEMO: std::clamp<uint32_t>(x+1, x+1, x+1) — degenerate clamp where
low == high == value, so the result must be x+1 by clamp's spec.

Same gap as algorithm_max: `std::clamp` lives in `<algorithm>`,
the tool treats it as a system-header extern and emits an
adversarial havoc override. The havoc accepts any return value,
so SAW disproves the equivalence against `add_one_spec`.

Requires C++17 for `std::clamp`. The runner forwards
`CxxStandard = 'c++17'` from cases.psd1.

Expected RESULT: DISPROVED (compositional havoc gap).
*/

#include <cstdint>
#include <algorithm>

uint32_t add_one(uint32_t x) {
    uint32_t y = x + 1;
    return std::clamp<uint32_t>(y, y, y);
}
