// E2E regression for issue #68: gen-verify must include the hidden sret
// return-pointer argument in llvm_execute_func when generating an
// experimental havoc spec for an external sub-callee that returns a
// struct by value.
//
// `canonicalize` is declared but never defined, so gen-verify places it
// in `specs_experimental/`.  Because it returns `Payload` by value (a
// 20-byte struct -- sret on every supported ABI), the generated havoc
// spec must include `result_ptr` as the first argument to llvm_execute_func.
// Without the fix SAW rejects the spec with "Argument 1 unspecified".
//
// `wrap_canonicalize` calls `canonicalize` without using its return value,
// so its own return value (x + 1) is fully deterministic and verifiable
// with SAW regardless of what the havoc'd sub-callee produces.
#include <cstdint>

struct Payload {
    uint32_t a;
    uint32_t b;
    uint32_t c;
    uint32_t d;
    uint32_t e;
};

// External sub-callee: declared only, no definition.
// Returns a struct by value -> sret ABI on all supported platforms.
Payload canonicalize(uint32_t x);

// Calls the sret sub-callee without using its return value.
// The return value (x + 1) does not depend on canonicalize's result,
// so SAW can verify this function even with a purely adversarial
// (havoc) spec for canonicalize.
uint32_t wrap_canonicalize(uint32_t x) {
    canonicalize(x);
    return x + 1;
}
