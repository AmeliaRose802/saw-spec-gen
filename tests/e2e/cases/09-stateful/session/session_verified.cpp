// session — a *multi-field* stateful post-state with kept (unchanged)
// fields. session_open sets the `isOpen` flag and must leave the rest of
// the object (the `tag` bytes) untouched.
//
// This is the "keep unchanged fields" case: with whole-object modelling
// the post-state simply pins the changed byte and re-uses the pre-state
// bytes for everything else (`[1] # drop`{1} pre`). No per-field flag is
// needed:
//
//   --out-buffer-param s=4  --cryptol-fn-out s=session_open_post
//
// All members are byte-granular (uint8), so the object models cleanly as
// a 4-byte buffer.
//
// verified : isOpen becomes 1; the three tag bytes are preserved.

#include <cstdint>

struct Session {
    std::uint8_t isOpen;   // offset 0
    std::uint8_t tag[3];   // offsets 1..3, opaque, must be preserved
};

std::uint8_t session_open(Session* s) noexcept {
    s->isOpen = 1;  // tag bytes left untouched.
    return 1;
}
