// partial_sret (disproved) — writes `a = x + 2` while the model asserts
// `x + 1`, so the 4-byte prefix mismatches. Confirms the partial-sret
// postcondition is not vacuous: it still catches a wrong prefix even
// though the trailing bytes are ignored.
//
// `Rec24` is 24 bytes so it is returned via the hidden sret pointer on
// both the Windows x64 and Linux SysV ABIs (see the verified variant for
// the cross-ABI size reasoning).

#include <cstdint>

struct Rec24 {
    std::uint32_t a;
    std::uint32_t tail[5];
};

Rec24 make_rec(std::uint32_t x) {
    Rec24 r;
    r.a = x + 2u;
    r.tail[0] = x;
    r.tail[1] = x;
    r.tail[2] = x;
    r.tail[3] = x;
    r.tail[4] = x;
    return r;
}
