#include <cstdint>
#include <cstdio>
#include <cstdlib>

/*
REGRESSION TEST (originally adversarial; now fixed).

The interface declares a `const` virtual method, but the data member
it modifies is marked `mutable`. C++ allows a `const` method to write
to `mutable` members. A sound havoc model must therefore treat `this`
as writable in this case.

A sound tool MUST report UNSAT (with a real counterexample): the
implementation `BadCounter::prepare` returns `x + 1 + 42`, violating
the spec `add_one(x) = x + 1`.

ORIGINAL BUG:
  The havoc model decided preserved-vs-havoced purely from the AST
  flag `is_const_method`, ignoring `mutable` on members. A `const`
  method got `llvm_alloc_readonly` for `this`, so `bias_` was modeled
  as preserved-at-0 — and equivalence checks passed for code that
  in reality returns `x + 43`.

FIX (applied):
  The AST parser now tracks `mutable`-qualified fields and propagates
  the flag through inheritance. For a `const` method on a class with
  any `mutable` field, `this` is treated as `Mutable`, and the havoc
  spec emits a full-class postcondition that preserves the vptr but
  writes fresh symbolics over each data member. The solver then
  finds the counterexample `this_bias__post != 0`.

Real-world relevance: `mutable` is pervasive in production C++
(caches, lazy init, ref counts, mutexes). Models that ignore it are
silently unsound on those patterns.
*/

class ICounter {
public:
    // `mutable` makes this member writable even from const methods.
    mutable uint32_t bias_ = 0;

    // Method is declared const — but `mutable` lets it modify bias_.
    virtual void prepare(uint32_t x) const = 0;
};

class OkCounter : public ICounter {
public:
    void prepare(uint32_t x) const override {
        printf("ok %u\n", x);
    }
};

class BadCounter : public ICounter {
public:
    void prepare(uint32_t x) const override {
        // LEGAL: bias_ is `mutable`, so this compiles inside a const method.
        bias_ = 42;
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    ICounter* counter;

    if (rand() % 2 == 0) {
        counter = new OkCounter();
    } else {
        counter = new BadCounter();
    }

    counter->prepare(x);

    return x + 1 + counter->bias_;
}
