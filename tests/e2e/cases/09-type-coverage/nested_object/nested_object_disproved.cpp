#include <cstdint>

// Buggy variant: returns the *first* field (`inner.a`, offset 0) instead
// of the modelled second field (`inner.b`, offset 4). Both are
// natural-aligned reads, so this still exercises the aligned-buffer fix —
// but the returned value diverges from the Cryptol model, so SAW returns
// DISPROVED. This proves the aligned-read proof is not vacuous.
struct Inner {
    std::uint32_t a;
    std::uint32_t b;
};

struct Outer {
    Inner inner;
};

std::uint32_t read_nested(const Outer* o) noexcept { return o->inner.a; }
