// counter (disproved) — the buggy body increments by *two*, so the
// post-state diverges from the `+1` Cryptol model. SAW returns the
// discriminating counterexample, proving the wide-field post-condition
// is not vacuous.
//
//   --out-buffer-param c=i32  --cryptol-fn-out c=counter_inc_post

#include <cstdint>

struct Counter {
    std::uint32_t n;
};

extern "C"
std::uint32_t counter_inc(Counter* c) noexcept {
    c->n = c->n + 2;  // BUG: should be + 1
    return 1;
}
