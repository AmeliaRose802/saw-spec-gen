/*
Inverted form of void_ptr_eq: returns 1 when pointers differ
(should be DISPROVED against the spec, which returns 1 iff equal).
*/

#include <cstdint>

uint32_t void_ptr_eq(const void* a, const void* b) {
    return a != b ? 1u : 0u;
}
