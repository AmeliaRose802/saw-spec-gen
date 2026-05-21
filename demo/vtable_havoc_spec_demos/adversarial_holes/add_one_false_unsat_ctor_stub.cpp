#include <cstdint>
#include <cstdio>

/*
REGRESSION TEST (originally adversarial; now fixed).

This function is provably correct:
  - Stack-allocates an OkLogger; ctor zero-initializes ILogger::count_
  - Calls a CONST virtual method (havoc model says `this` is preserved)
  - Returns x + 1 + count_  =  x + 1 + 0  =  x + 1

A sound tool MUST report SAT (verified).

ORIGINAL BUG:
  Tool reported UNSAT with a fake counterexample. The auto-generated
  `<class>_ctor_override` allocated only 8 bytes (just the vptr) and
  never applied the default member initializers. Reads of `count_`
  at offset 8 returned fresh symbolic values, fabricating a fake CEX
  even though the compiled binary always returned the correct value.

FIX (applied):
  Ctor overrides now allocate the full class layout (`llvm_alloc
  (llvm_alias "class.<Layout>")`) and write each data member's
  in-class initializer via `llvm_struct_value` in the postcondition.
  See `demo/vtable_havoc_spec_demos/adversarial_holes/README.md`.
*/

class ILogger {
public:
    uint32_t count_ = 0;     // initialized to 0 by every subclass's ctor

    // CONST virtual method — havoc model says `this` is preserved.
    virtual void log(uint32_t x) const = 0;
};

class OkLogger : public ILogger {
public:
    void log(uint32_t x) const override {
        printf("log %u\n", x);
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    ILogger* l = new OkLogger();   // ctor zero-initializes count_

    l->log(x);                     // const method — count_ preserved at 0

    // count_ is provably 0, so this returns x + 1.
    return x + 1 + l->count_;
}
