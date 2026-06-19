/*
DEMO: std::string::operator[](pos) and at(pos) leave size unchanged.

operator[] and at() return char& (i8* in LLVM IR). Our byte-view
index override family declares a fresh symbolic 'idx' argument,
returns a fresh byte pointer, and imposes no constraint on the
string's size field. Hence s.size() after the subscript call is
still equal to the value written by resize().

The returned byte is read into a volatile-discarded local so the
compiler emits the actual operator[] call.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 2u);
    char c = s[0];
    (void)c;
    return static_cast<uint32_t>(s.size());
}
