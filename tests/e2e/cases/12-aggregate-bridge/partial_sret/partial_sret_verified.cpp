// partial_sret — exercises `sret_assert_bytes`: assert only the
// meaningful prefix of an aggregate sret return, ignoring the rest.
//
// `Rec24` is 24 bytes, so it is returned via the hidden sret pointer on
// both the Windows x64 ABI (struct > 8 bytes) and the Linux SysV ABI
// (struct > 16 bytes). A 16-byte struct would be returned in registers
// on SysV (no sret), so the struct is deliberately over 16 bytes to keep
// this case cross-platform. The model constrains only the first field
// `a`; `sret_assert_bytes = 4` makes the tool emit
// `llvm_points_to_at_type result_ptr (llvm_array 4 (llvm_int 8)) ...`,
// so the trailing 20 bytes are never read. (A plain full-width assertion
// is impossible here anyway: a [4][8] model is not memory-compatible
// with the [24][8] sret allocation.)

#include <cstdint>

struct Rec24 {
    std::uint32_t a;
    std::uint32_t tail[5];
};

Rec24 make_rec(std::uint32_t x) {
    Rec24 r;
    r.a = x + 1u;
    r.tail[0] = x;
    r.tail[1] = x;
    r.tail[2] = x;
    r.tail[3] = x;
    r.tail[4] = x;
    return r;
}
