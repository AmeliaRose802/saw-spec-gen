// session (disproved) — buggy open that clobbers a field which the
// post-state model says must be kept.
//
// session_open here also zeroes the first tag byte, but the whole-object
// model (`session_open_post`) preserves every tag byte from the
// pre-state. For any pre-state with a non-zero tag[0], SAW finds the
// mismatch — proving the kept-field bytes are genuinely constrained, not
// left free.
//
// The return value (1) still matches, so the discriminating check is the
// preserved-field portion of the four-byte post-state.

#include <cstdint>

struct Session {
    std::uint8_t isOpen;
    std::uint8_t tag[3];
};

extern "C"
std::uint8_t session_open(Session* s) noexcept {
    s->isOpen = 1;
    s->tag[0] = 0;  // BUG: wipes a tag byte that should be preserved.
    return 1;
}
