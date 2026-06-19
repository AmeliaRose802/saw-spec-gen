/*
DEMO: std::string::data() under the SSO branch and the heap
branch (saw_spec_gen-xzg).

libstdc++ `basic_string` is short-string-optimized: for sizes
<= 15 chars, `data()` returns a pointer into the embedded
`_M_local_buf` (field 2 of the IR struct); for longer strings,
it returns the heap pointer stashed in `_M_dataplus._M_p`
(field 0). The two branches return pointers backed by storage
in completely different fields of the container.

The functional override for `basic_string::data()` lives in
`src/emit/saw_emit/bitcode_overrides_functional_string.rs`. It
implements xzg's *model-agnostic* option (a): always allocate
a fresh symbolic byte buffer, bind `field 0` to it, and return
that pointer. The post-state therefore over-approximates BOTH
SSO branches uniformly — any read through the returned pointer
sees fully symbolic bytes, regardless of which branch the real
implementation would have taken.

This test exercises the override on a symbolic `x`, so SAW
explores all reachable sizes — including both `x + 1 <= 15`
(SSO) and `x + 1 > 15` (heap). The equivalence to
`add_one x = x + 1` holds for every branch because the
returned pointer is unused: we only read it back as `(void)p;`
to force the override to fire.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    auto p = s.data();
    (void)p;
    return static_cast<uint32_t>(s.size());
}
