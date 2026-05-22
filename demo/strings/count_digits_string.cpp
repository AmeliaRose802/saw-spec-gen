/*
DEMO (ROADMAP -- currently NOT verifiable end-to-end).

count_digits over a `const std::string&` parameter.

This is the same operation as count_digits_cstr.cpp but takes a
real C++ std::string by reference. It is included as a deliberate
target for future work on gen-verify's STL support, not as a
passing test case.

What works today
----------------
verify.ps1 now runs a path-based AST pre-filter that strips every
top-level declaration whose source file isn't under the demo's
directory. For this file the raw clang AST dump is ~328 MB; after
the filter only the user's `count_digits` declaration survives (8
"no-loc" builtin nodes are passed through). gen-verify therefore
loads the AST, finds the function, and emits a verify.saw. Step 2.5
of verify.ps1 reports something like:

    Filter result: kept 1, dropped 7298, no-loc 8

That filter is purely path-prefix based -- no allowlist of
toolchain include directories. The same mechanism handles GCC,
clang, MSVC, MinGW, and any third-party headers transparently.

What still doesn't work
-----------------------
The Cryptol spec in count_digits_spec.cry is written for the
fixed-length 8-byte buffer case (`[8][8] -> [32]`). A correct spec
for the std::string flavour would have to describe how a
`basic_string` is *laid out in memory* on MSVC -- the 16-byte SSO
union, the size_t length, the size_t capacity -- and that's exactly
what gen-verify can't synthesise on its own today. Even after the
AST filter the equivalence check fails with:

    Type mismatch:
      Expected type: 8
      Inferred type: 32
    When checking type of function argument

which is Cryptol's polite way of saying "you handed me a 32-bit
length where I wanted an 8-byte vector".

What a successful run will eventually require
---------------------------------------------
* A `std::basic_string<char>` layout recogniser in the AST emitter
  so the `s_ptr` parameter is allocated as a 32-byte struct (SSO
  union + size + capacity) instead of a single byte.
* A library of MSVC STL overrides (`size`, `operator[]`, `c_str`,
  default ctor / dtor) shipped under lib/, in the same spirit as
  the MSP repo's lib/msvc_string_overrides.saw.
* A Cryptol spec that operates on the assumed-layout struct, not on
  a raw `[N][8]` buffer.
* A loop-bound pragma (or `_In_reads_(s.size())`-style annotation)
  so the verifier knows how many iterations to unroll once the
  loop bound is a symbolic `size_t`.

Registered in tests/saw_demos/cases.psd1 with `Expected =
'UNKNOWN'` so the suite tracks the demo's current failure mode --
when we eventually make it pass, the harness will refuse to
continue claiming UNKNOWN and the case will need to be flipped to
SAT.
*/

#include <cstdint>
#include <string>

uint32_t count_digits(const std::string& s) {
    uint32_t n = 0;
    for (size_t i = 0; i < s.size(); i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        if (b >= 0x30 && b <= 0x39) {
            n += 1;
        }
    }
    return n;
}
