/*
DEMO (ROADMAP -- currently NOT verifiable end-to-end).

count_digits over a `const std::string&` parameter.

This is the same operation as count_digits_cstr.cpp but takes a
real C++ std::string by reference. It is included as a deliberate
target for future work on gen-verify's STL support, not as a
passing test case.

Why it doesn't pass today
-------------------------
0. The AST blows up. Simply running `clang -ast-dump=json` on this
   translation unit produces ~328 MB of JSON -- including <string>
   pulls in the entire MSVC <xstring>/<xmemory>/<xutility> chain
   plus their templated dependencies. gen-verify currently refuses
   to load AST files over 100 MB. The first thing a real STL story
   needs is an AST filter (clang -ast-dump-filter=count_digits, or
   a jq post-process) so only the target's declarations + the
   handful of std::basic_string members it actually touches are
   kept.

1. MSVC `std::basic_string<char>` is a heap-backed type with a
   small-string-optimisation (SSO) union. Its in-memory layout
   under x86_64 MSVC is roughly:

       struct {
         union {
           char        _Buf[16];     // SSO: inline storage
           char*       _Ptr;         // heap: separately allocated
         } _Bx;
         size_t        _Mysize;      // current length
         size_t        _Myres;       // current capacity
       };

   gen-verify currently cannot synthesise an opaque allocation for
   this layout: it would have to know to allocate either a 16-byte
   SSO buffer *or* a separate heap chunk, and to thread the chosen
   discriminant through `size()` / `operator[]` calls.

2. The member functions used here (`.size()`, `operator[]`) are
   non-trivial inline templates. clang emits real calls to
   `std::basic_string::size()` and `operator[]`. Without overrides
   for those, SAW fails with "Could not find definition for
   function" on the first one it hits.

3. Once the loop bound is a symbolic `size_t` rather than the
   compile-time constant 8, SAW also needs a loop-bound hint or a
   fixpoint command -- gen-verify doesn't emit one today.

What a successful run will eventually require
---------------------------------------------
* AST filtering so the JSON dump stays under the size limit.
* Recognising `std::basic_string<char>` (and `std::string` typedef)
  in the AST as a known STL container, and synthesising a
  parameterised "logical view" of its bytes for the spec.
* A library of MSVC STL overrides (`size`, `operator[]`, `c_str`,
  default ctor / dtor) shipped under lib/, in the same spirit as
  the MSP repo's lib/msvc_string_overrides.saw.
* A loop-bound pragma (or `_In_reads_(s.size())`-style annotation)
  so the verifier knows how many iterations to unroll.

Expected outcome today: verify.ps1 aborts in Step 3 because the
AST dump exceeds gen-verify's 100 MB safety limit. The case is
intentionally NOT registered in tests/saw_demos/cases.psd1.
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
