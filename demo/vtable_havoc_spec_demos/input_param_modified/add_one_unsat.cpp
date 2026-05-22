#include "validator.h"

/*
DEMO: Unsat (fails) — input variable is modified through a non-const pointer.

The interface in validator.h declares:
    virtual void validate(uint32_t* val) = 0;

The validator is passed in as an IValidator* parameter, so SAW cannot
see the concrete implementation. Its vtable havoc model assumes the
worst: a non-const pointer parameter could be written to. After the
call, `input` is unconstrained — so `input + 1` may not equal `x + 1`,
and the equivalence check fails.

FIX: declare `validate(const uint32_t* val)` (see add_one_sat.cpp),
     or in production code use the _In_ SAL annotation.
*/

// Spec: add_one(x) = x + 1
uint32_t add_one(IValidator* v, uint32_t x) {
    uint32_t input = x;

    v->validate(&input);

    return input + 1;
}
