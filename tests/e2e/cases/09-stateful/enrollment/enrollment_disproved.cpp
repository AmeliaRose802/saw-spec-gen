// enrollment (disproved) — same named heterogeneous struct allocation
// as enrollment_verified.cpp, but the body corrupts the byte field
// while updating the wide i64 field. The Cryptol model preserves
// `engaged`, so SAW finds a counterexample.
//
//   --out-buffer-param k=struct:EnrollmentKey
//   --cryptol-fn-out   k=enroll_key_post

#include <cstdint>

struct EnrollmentKey {
    std::uint8_t engaged;
    std::int64_t createdAt;
};

std::uint32_t enroll_key(EnrollmentKey* k) noexcept {
    std::uint8_t engaged = k->engaged;
    k->createdAt = 42;
    k->engaged = engaged ^ 1; // BUG: model keeps engaged unchanged
    return static_cast<std::uint32_t>(engaged);
}
