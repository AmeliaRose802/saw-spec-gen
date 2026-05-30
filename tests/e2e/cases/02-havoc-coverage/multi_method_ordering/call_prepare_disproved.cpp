#include "iprocessor.h"

/*
TEST: call ONLY prepare() (vtable slot 2) — EXPECTED DISPROVED

prepare is the ONE non-const method on the interface, so under
the havoc model its spec is allowed to mutate `this` — including
the `bias_` data member.  After the call, bias_ is a fresh symbolic
value (could be 42, could be anything), so the result
    x + 1 + bias_
is NOT necessarily equal to add_one_spec(x) = x + 1.

If the spec generator misorders the vtable and accidentally binds a
const method's (preserving) spec to slot 2, then this proof would
mysteriously SUCCEED — a soundness hole.
*/

uint32_t add_one(uint32_t x) {
    IProcessor* p;
    if (rand() % 2 == 0) {
        p = new GoodProcessor();
    } else {
        p = new BadProcessor();
    }

    p->prepare(x);           // non-const → bias_ HAVOCED

    return x + 1 + p->bias_; // bias_ unconstrained → counterexample
}
