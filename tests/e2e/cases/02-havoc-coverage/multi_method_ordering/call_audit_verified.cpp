#include "iprocessor.h"

/*
TEST: call ONLY audit() (vtable slot 1)

audit is `const`, so its havoc spec preserves `this` (and bias_).
The result should be x + 1 + 0 == x + 1, matching the Cryptol spec.

If the spec generator misorders the vtable and binds "prepare"
(slot 2, non-const) to slot 1, then this proof would FAIL.
*/

uint32_t add_one(uint32_t x) {
    IProcessor* p;
    if (rand() % 2 == 0) {
        p = new GoodProcessor();
    } else {
        p = new BadProcessor();
    }

    p->audit(x);             // const → bias_ preserved

    return x + 1 + p->bias_; // bias_ == 0 → x + 1
}
