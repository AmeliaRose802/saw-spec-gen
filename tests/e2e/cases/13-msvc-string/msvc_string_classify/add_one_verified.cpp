/*
Regression test for MSVC basic_string method classification (issue #73).

On MSVC, std::basic_string methods carry MSVC-mangled names such as
  `?size@?$basic_string@DU?$char_traits@D@std@@...` (size)
  `??0?$basic_string@DU?$char_traits@D@std@@...`    (ctor)
Before the fix, classify_basic_string_msvc was missing so these names
fell through to unoptimised havoc specs with wrong return types,
causing SAW type-mismatch errors at override registration time.

On Linux (Itanium ABI) clang generates `_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE6resizeEm`
etc.; the Itanium classification path is exercised instead and this
test exercises the full pipeline end-to-end.  The MSVC-specific
classification is verified separately by unit tests
(bitcode_overrides_tests_msvc.rs).

RESULT: VERIFIED — the resize(x+1)/size() round-trip always yields x+1.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(static_cast<std::size_t>(x) + 1u);
    return static_cast<uint32_t>(s.size());
}
