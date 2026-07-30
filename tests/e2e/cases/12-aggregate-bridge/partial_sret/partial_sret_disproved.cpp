// partial_sret (disproved) — writes `a = x + 2` while the model asserts
// `x + 1`, so the 4-byte prefix mismatches. Confirms the partial-sret
// postcondition is not vacuous: it still catches a wrong prefix even
// though the trailing bytes are ignored.

#include <cstdint>

struct Rec16 {
    std::uint32_t a;
    std::uint32_t tail[3];
};

Rec16 make_rec(std::uint32_t x) {
    Rec16 r;
    r.a = x + 2u;
    r.tail[0] = x;
    r.tail[1] = x;
    r.tail[2] = x;
    return r;
}
