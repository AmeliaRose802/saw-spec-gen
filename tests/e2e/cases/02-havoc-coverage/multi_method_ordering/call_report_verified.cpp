#include "iprocessor.h"

/*
TEST: call ONLY report() (vtable slot 3)

report is `const`, so its havoc spec preserves `this` (and bias_).
The result should be x + 1 + 0 == x + 1, matching the Cryptol spec.

slot 3 is the LAST slot on IProcessor.  If the spec generator
accidentally writes only N-1 slots into the vtable, calling
report() would dispatch to garbage and either fail or silently
mis-resolve.
*/

uint32_t add_one(uint32_t x) {
    IProcessor* p;
    if (rand() % 2 == 0) {
        p = new GoodProcessor();
    } else {
        p = new BadProcessor();
    }

    p->report(x);            // const → bias_ preserved

    return x + 1 + p->bias_; // bias_ == 0 → x + 1
}
