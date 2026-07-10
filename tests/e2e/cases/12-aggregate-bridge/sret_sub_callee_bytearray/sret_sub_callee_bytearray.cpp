// E2E regression for the hmac_sha256 sub-callee arg-mismatch surface
// (docs/22-isvalidsignature-hmac-subcallee-arg-mismatch.md).
//
// Unlike `sret_sub_callee_havoc` (which returns a plain `struct` the AST
// classifies as `TypeInfo::Struct` -> sret), this sub-callee returns a
// `std::array<uint8_t, 32>`. The clang AST does NOT classify that return
// as an sret aggregate (it is not a bare `struct` and its opaque form
// carries a non-namespaced element name), so the AST heuristic leaves
// `is_sret = false`. The MSVC x64 ABI still lowers the 32-byte return
// through a hidden `sret` output pointer:
//
//   declare void @"?hmac_sha256..."(ptr sret(%"class.std::array"), i32, i32)
//
// Without `ensure_sret_from_ir` the generated havoc override emits only
// two arguments in `llvm_execute_func`, and SAW rejects it with
// "Argument 2 unspecified". Recovering the sret from the authoritative
// IR signature restores the correct 3-argument call shape.
//
// `wrap_hmac` calls the sret sub-callee but ignores its return value, so
// its own return value (x + 1) is fully deterministic and SAW-verifiable
// regardless of what the havoc'd sub-callee produces.
#include <array>
#include <cstdint>

using Digest = std::array<uint8_t, 32>;

// External sub-callee: declared only, no definition. Returns a 32-byte
// aggregate by value -> hidden sret output pointer on MSVC x64.
Digest hmac_sha256(uint32_t key, uint32_t msg);

// Calls the sret sub-callee without using its return value.
uint32_t wrap_hmac(uint32_t x) {
    hmac_sha256(x, x);
    return x + 1;
}
