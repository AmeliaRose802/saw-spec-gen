// Smoke test #1: in-module body-having callee (inline header helper).
// MUST be executed concretely, NOT havoc'd. Mirrors the original
// DISPROVED case that motivated the override-determination fix.
#include <cstdint>

inline uint32_t double_it(uint32_t x) { return x + x; }

uint32_t use_helper(uint32_t x) { return double_it(x); }
