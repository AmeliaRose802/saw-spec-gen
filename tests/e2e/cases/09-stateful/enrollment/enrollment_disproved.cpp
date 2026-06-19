// enrollment (disproved) — same heterogeneous struct out-buffer shape
// as enrollment_verified.cpp, but the body has a bug: in addition to
// bumping `id` it also flips the `engaged` flag. The Cryptol model
// preserves `engaged`, so the post-states diverge and SAW returns the
// discriminating counterexample — proving the whole-object struct
// post-condition is not vacuous.
//
//   --out-buffer-param k={i32,i8}  --cryptol-fn-out k=enroll_key_post

#include <cstdint>

struct EnrollmentKey {
    std::uint32_t id;
    std::uint8_t engaged;
};

std::uint32_t enroll_key(EnrollmentKey* k) noexcept {
    k->id = k->id + 1;
    k->engaged = k->engaged ^ 1; // BUG: model keeps engaged unchanged
    return 1;
}
