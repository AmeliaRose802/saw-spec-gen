/*
DEMO: std::string::empty() returns true for a default-constructed string.

The default constructor (BasicStringCtorDefault) sets the size field
to 0. The empty() override (BasicStringEmpty) reads that field and
returns sz == 0 as i1. Together they guarantee that a freshly
default-constructed string is always empty, so the branch that
computes x + 1 is always taken.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    return s.empty() ? x + 1u : x;
}
