#include <cstdint>
#include <cstdlib>
#include "../saw_sal.h"

/*
DEMO: VERIFIED because `__restrict` + `_In_`/`_Out_` annotations let the spec
generator emit precise pre/postconditions for non-aliasing buffers.

`transform(_In_ in, _Out_ out)` declares the input as preserved
(read-only) and the output as write-only. `__restrict` tells the C/C++
toolchain that the two pointers never alias, so a write to `*out`
cannot perturb `*in`.

For SAW that means:
  - precondition  `*in = in_val` (in_val is the fresh symbolic input)
  - postcondition `*out = {{ add_one_spec in_val }}` (= in_val + 1)

The havoc spec is therefore tight enough to prove
`add_one(x) == add_one_spec(x)`.
*/

class ITransform {
public:
    virtual void transform(_In_  const uint32_t* __restrict in,
                           _Out_       uint32_t* __restrict out) = 0;
};

class OkTransform : public ITransform {
public:
    void transform(_In_  const uint32_t* __restrict in,
                   _Out_       uint32_t* __restrict out) override {
        *out = *in + 1;
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    ITransform* t = new OkTransform();
    uint32_t result;
    t->transform(&x, &result);
    return result;
}
