#include <cstdint>
#include <cstdlib>

/*
DEMO: DISPROVED (DISPROVED) because the interface has neither `_In_`/`_Out_`
annotations nor `__restrict`.

Without `_Out_`, the havoc spec generator falls back to "this pointer is
mutable; the solver picks ANY final value for `*out`". There is no link
back to the Cryptol spec, so `add_one(x) == add_one_spec(x)` cannot be
proved — the solver finds a counterexample where the override writes
anything other than `x + 1`.

FIX: see `add_one_verified.cpp` in the same directory — it adds `_In_`/`_Out_`
SAL annotations (via `saw_sal.h`) plus `__restrict`.
*/

class ITransform {
public:
    virtual void transform(const uint32_t* in, uint32_t* out) = 0;
};

class OkTransform : public ITransform {
public:
    void transform(const uint32_t* in, uint32_t* out) override {
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
