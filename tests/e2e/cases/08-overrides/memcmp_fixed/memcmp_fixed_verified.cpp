// memcmp_fixed — exercises the faithful fixed-length `memcmp` override.
//
// `memcmp` is a libc extern with no body in the bitcode, so
// saw-spec-gen's extern_override_scan wraps it in an
// `llvm_unsafe_assume_spec`. The OLD behaviour havoc'd the return
// value (`rv <- llvm_fresh_var`), which let the solver pick `rv == 0`
// for UNEQUAL buffers (a false DISPROVED) or a non-zero `rv` for equal
// buffers (a false VERIFIED).
//
// Because every call site here passes the compile-time-constant length
// 8, the scan extracts it and emits a FAITHFUL spec: the override
// returns 0 exactly when the two 8-byte buffers are equal. This case
// proves `tags_equal(a, b) == (a == b)`.

#include <cstdint>
#include <cstring>

bool tags_equal(const std::uint8_t* a, const std::uint8_t* b) {
    return std::memcmp(a, b, 8) == 0;
}
