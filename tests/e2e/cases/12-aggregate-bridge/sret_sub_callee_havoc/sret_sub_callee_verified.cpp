// E2E regression for issue #68: gen-verify must include the hidden sret
// return-pointer argument in llvm_execute_func when generating an
// experimental havoc spec for an external sub-callee that returns a
// struct by value.
//
// `canonicalize` is declared but never defined, so gen-verify places it
// in `specs_experimental/`.  Because it returns `Payload` by value (a
// 20-byte struct -- sret on every supported ABI), the generated spec
// must allocate `result_ptr` and pass it as the first argument.
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

// Top-level function under verification: reads a field from the sret return.
uint32_t wrap_canonicalize(uint32_t x) {
    Payload p = canonicalize(x);
    return p.a;
}
