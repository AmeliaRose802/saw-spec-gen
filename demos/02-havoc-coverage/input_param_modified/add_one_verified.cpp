#include "validator_const.h"

/*
DEMO: Verified (verifies) because the validator's pointer parameter is const.

The interface is declared in validator_const.h:
    virtual void validate(const uint32_t* val) = 0;

The validator is passed in as an IValidator* parameter, so its concrete
implementation is opaque to this translation unit (and to SAW). SAW
applies its vtable havoc model — but the const pointee tells the model
that the call cannot write through `val`. So `input` after the call
still equals `x`, and `input + 1 == x + 1` holds for all x.

In production code, the SAL annotation _In_ has the same effect:
    virtual void validate(_In_ uint32_t* val) = 0;
*/

// Spec: add_one(x) = x + 1
uint32_t add_one(IValidator* v, uint32_t x) {
    uint32_t input = x;

    // const pointer — SAW knows *input is unchanged
    v->validate(&input);

    // input is guaranteed to still equal x
    return input + 1;
}
