/*
DEMO: std::pair as a value type. Constructing `std::pair{x+1, x}`
on the stack and returning `.first` should compile to a plain
struct allocation + GEP + load with no external calls — the pair
constructor is a header-only `constexpr` template that clang
inlines aggressively.

With the Crucible-safety analyzer (saw_spec_gen-9x5), even when
the pair constructor survives as a callable symbol (e.g. at -O0),
its body is allocator-free arithmetic + memory ops and gets
symbolically executed instead of havoced.

Expected RESULT: VERIFIED. Set
`SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` to bisect.
*/

#include <cstdint>
#include <utility>

uint32_t add_one(uint32_t x) {
    std::pair<uint32_t, uint32_t> p{x + 1u, x};
    return p.first;
}
