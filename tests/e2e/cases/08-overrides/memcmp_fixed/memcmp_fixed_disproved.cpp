// memcmp_fixed (disproved) — the return is INVERTED (`!= 0`), so
// `tags_equal(a, b)` computes `a != b` while the Cryptol model says
// `a == b`. With the faithful fixed-length memcmp override this is a
// genuine DISPROVED (counterexample: two equal buffers). Under the old
// havoc'd `rv` this case could pass vacuously — the non-vacuity guard.

#include <cstdint>
#include <cstring>

bool tags_equal(const std::uint8_t* a, const std::uint8_t* b) {
    return std::memcmp(a, b, 8) != 0;
}
