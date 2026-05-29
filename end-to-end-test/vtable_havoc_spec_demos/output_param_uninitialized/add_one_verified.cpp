#include <cstdint>
#include <cstdlib>
#include "../saw_sal.h"

/*
DEMO: VERIFIED because the _Out_ annotation guarantees the buffer is written.

`compute` is declared with `_Out_ uint32_t* buf`. The SAW spec generator
treats `_Out_` parameters specially:

  1. NO precondition that `*buf` is initialized (the caller may pass
     uninitialized memory — exactly what happens here).
  2. Postcondition: `*buf` is set to the value computed by the
     user-supplied Cryptol spec applied to the inputs — here,
     `{{ add_one_spec x }}` (= `x + 1`).

The `_Out_` annotation is standard Microsoft SAL, redefined to
`__attribute__((annotate("_Out_")))` by `saw_sal.h` so clang exposes it
in the JSON AST.
*/

class IComputer {
public:
    virtual void compute(uint32_t x, _Out_ uint32_t* buf) = 0;
};

class OkComputer : public IComputer {
public:
    void compute(uint32_t x, _Out_ uint32_t* buf) override {
        *buf = x + 1;
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    IComputer* c = new OkComputer();
    uint32_t result;  // uninitialized — `_Out_` guarantees it is written
    c->compute(x, &result);
    return result;
}
