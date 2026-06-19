/*
DEMO: std::string::reserve() does not affect size().

reserve(n) adjusts capacity but leaves the logical size untouched.
The BasicStringReserve override declares a fresh 'n' arg and has no
post-state llvm_points_to, confirming it makes no observable change
to the size field. After reserve() the size is still x + 1 as set
by resize().
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    s.reserve(x + 128u);
    return static_cast<uint32_t>(s.size());
}
