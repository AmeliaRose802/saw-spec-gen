// partial_sret — exercises `sret_assert_bytes`: assert only the
// meaningful prefix of an aggregate sret return, ignoring the rest.
//
// `Rec16` is returned via the hidden sret pointer (16 bytes > 8). The
// model constrains only the first field `a`; `sret_assert_bytes = 4`
// makes the tool emit `llvm_points_to_at_type result_ptr (llvm_array 4
// (llvm_int 8)) ...`, so the trailing 12 bytes are never read. (A plain
// full-width assertion is impossible here anyway: a [4][8] model is not
// memory-compatible with the [16][8] sret allocation.)

#include <cstdint>

struct Rec16 {
    std::uint32_t a;
    std::uint32_t tail[3];
};

Rec16 make_rec(std::uint32_t x) {
    Rec16 r;
    r.a = x + 1u;
    r.tail[0] = x;
    r.tail[1] = x;
    r.tail[2] = x;
    return r;
}
