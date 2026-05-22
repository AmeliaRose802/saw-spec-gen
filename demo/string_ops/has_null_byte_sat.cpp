/*
DEMO: SWAR null-byte detection -- a real string operation.

This is the same bit-trick that lives inside glibc strlen,
AddressSanitizer's shadow checks, and dozens of memchr/strchr
variants. Given a 64-bit word `w` interpreted as 8 packed bytes
(i.e. an 8-character fixed-length string slice), it answers
"does any byte equal zero?" branchlessly, in O(1) machine
instructions.

Why is this a verification problem at all? Because the bit-trick is
notoriously subtle:

    has  = (w - 0x0101010101010101) & ~w & 0x8080808080808080;

Read it slot-by-slot. For each of the 8 byte lanes:
  * Subtracting 1 from a zero byte underflows and sets the top bit
    (0x00 - 1 == 0xFF, top bit on).
  * `& ~w` blanks any lane whose original top bit was already on,
    preventing a false positive when the byte was, say, 0x80.
  * `& 0x80...` keeps only the high bit of each lane.

Convince yourself with byte = 0x00: 0x00 - 1 = 0xFF, ~0x00 = 0xFF,
0xFF & 0xFF & 0x80 = 0x80 -- detected. With byte = 0x80:
0x80 - 1 = 0x7F, ~0x80 = 0x7F, 0x7F & 0x7F & 0x80 = 0x00 -- not
detected. With byte = 0x01: 0x01 - 1 = 0x00 -- not detected.

The Cryptol spec in has_null_byte_spec.cry just says "split w into 8
bytes and OR together (b == 0)". SAW must prove the branchless
formula above is bit-for-bit equivalent to that direct definition,
for every one of the 2^64 possible inputs. The proof is fully
automatic via z3.

Expected RESULT: SAT (i.e. matches the Cryptol spec).
*/

#include <cstdint>

uint32_t has_null_byte(uint64_t w) {
    // Mycroft 1987. Returns 1 iff any byte in w is 0x00, else 0.
    constexpr uint64_t LO = 0x0101010101010101ULL;
    constexpr uint64_t HI = 0x8080808080808080ULL;
    return ((w - LO) & ~w & HI) != 0 ? 1u : 0u;
}
