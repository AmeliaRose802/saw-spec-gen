/*
Regression test for extern-callee aggregate return-type handling (issue #73).

On MSVC, functions returning small structs use a `[N x i8]` aggregate
return in LLVM IR (instead of the sret pointer used on Itanium).
Before the fix, split_whitespace split `[16 x i8]` into three tokens
and ir_return_setup had no ByteArray case, so the generated override
was missing its llvm_return, causing SAW type-mismatch errors.

On Linux (Itanium ABI) the same pattern is expressed as an sret
parameter; the test exercises that path end-to-end and the MSVC
`[N x i8]` return handling is verified by unit tests
(bitcode_overrides_tests_msvc.rs, extern_override_scan_tests_compound.rs).

`get_token` is declared but never defined so gen-verify places it in
`specs_experimental/` as a havoc spec.  `add_one` discards the struct
return value, so x + 1 is fully deterministic and SAW-verifiable.

RESULT: VERIFIED.
*/

#include <cstdint>

struct Token {
    uint32_t a;
    uint32_t b;
    uint32_t c;
    uint32_t d;
    uint32_t e;
};

// External sub-callee: declared only, no definition.
// Returns a 20-byte struct by value -> sret ABI on all supported platforms.
Token get_token(uint32_t id);

uint32_t add_one(uint32_t x) {
    (void)get_token(x);
    return x + 1;
}
