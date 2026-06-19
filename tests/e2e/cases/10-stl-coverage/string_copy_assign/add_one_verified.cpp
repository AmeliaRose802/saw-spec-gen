/*
DEMO: copy-assign (operator=) threads size from source to dest.

The BasicStringAssignStr / BasicStringOpEqStr override binds the
source string's size field to sz_src in its pre-state and writes
sz_src into the destination's size field in the post-state. This
couples the resize() of 'src' to the size() of 'dst' across the
copy-assignment, enabling a full end-to-end VERIFIED proof without
any content tracking.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string src;
    src.resize(x + 1u);
    std::string dst;
    dst = src;
    return static_cast<uint32_t>(dst.size());
}
