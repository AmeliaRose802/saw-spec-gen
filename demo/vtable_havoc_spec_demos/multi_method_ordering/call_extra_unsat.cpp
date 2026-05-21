#include "iprocessor.h"

/*
TEST: call ONLY extra() on the subclass (vtable slot 4) — EXPECTED UNSAT

extra() is a NEW virtual method declared on BadProcessor that does
NOT exist on the IProcessor parent.  It occupies vtable slot 4
(immediately after the 4 inherited slots).

Because the call goes through `BadProcessor*`, dispatch resolves
to BadProcessor's own vtable.  The spec generator must:
  - emit a stub for badprocessor_extra
  - place it at slot 4 of badprocessor_vtable
  - leave the inherited slots 0..3 wired to the originating stubs

extra() is non-const, so its havoc spec havocs `this->bias_`,
which makes the proof fail (counterexample expected).

If the spec generator forgets the extra-method slot, this verify
either errors out (resolve failure) or trivially succeeds (no spec
constrains the result) — both are bugs.
*/

uint32_t add_one(uint32_t x) {
    BadProcessor* p = new BadProcessor();

    p->extra(x);             // non-const, subclass-only → bias_ HAVOCED

    return x + 1 + p->bias_; // bias_ unconstrained → counterexample
}
