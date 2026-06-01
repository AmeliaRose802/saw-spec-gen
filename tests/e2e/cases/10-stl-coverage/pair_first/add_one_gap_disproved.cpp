/*
DEMO: std::pair as a value type. Constructing `std::pair{x+1, x}`
on the stack and returning `.first` should compile to a plain
struct allocation + GEP + load with no external calls — the pair
constructor is a header-only `constexpr` template that clang
inlines aggressively.

This case tests whether saw-spec-gen handles the LLVM struct that
backs `std::pair<uint32_t, uint32_t>` cleanly. If the constructor
remains as a callable symbol in the bitcode (rather than being
fully inlined), it will hit the same havoc gap as the algorithm
cases.

Expected RESULT: depends on clang's inlining decisions at -O0 —
documented as the experimental side of the suite.
*/

#include <cstdint>
#include <utility>

uint32_t add_one(uint32_t x) {
    std::pair<uint32_t, uint32_t> p{x + 1u, x};
    return p.first;
}
