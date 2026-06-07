/*
DEMO: std::string::resize + size() — the canonical "smart
container" workflow for the std::string family. Allocates a
heap buffer of length (x + 1) and returns that length back to
the caller.

Now VERIFIED: saw-spec-gen-i47 / `bitcode_overrides_functional`
emits functional SAW specs for `std::basic_string::{C1Ev, C2Ev,
D1Ev, D2Ev, size, resize, data}` instead of the previous blanket
havoc. The functional `resize(n)` spec writes `n` into field 1
(`_M_string_length`) of the IR struct
`class.std::__cxx11::basic_string`; the functional `size()` spec
binds the same field in its pre-state and returns it. Together
they couple the `resize(x + 1)` and `s.size()` calls so the
equivalence to `add_one_spec x = x + 1` proves.

Field-index discovery walks the LLVM IR struct table at run time
(`discover_string_layout`) and falls back to the default havoc
emitter if the IR does not declare any recognizable basic_string
struct, so the fix is robust across libstdc++ vs libc++ vs MSVC.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    return static_cast<uint32_t>(s.size());
}
