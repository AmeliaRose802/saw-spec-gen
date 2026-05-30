// DEMO: a function that throws a C++ exception on a "bad" input,
// but catches it locally and still returns the correct value.
//
// add_one(x) computes x + 1. On x == 42 it raises `throw BadInput{}`
// inside a try block, which the same function catches via
// `catch (const BadInput&)`. The catch handler does an observable
// side effect (a volatile write) but does NOT alter the return value:
// add_one always returns x + 1.
//
// The throw is inline (not in a helper) because exception-lower's
// per-function error-flag mechanism does not propagate across call
// boundaries when SAW executes the callee inline. The exception
// type is a struct (not `int`) so that the typeinfo is defined
// in-module, avoiding SAW's limitation with external typeinfo
// symbols like `@_ZTIi`.
//
// Expected RESULT: VERIFIED by z3.

#include <cstdint>

struct BadInput {};

static volatile uint32_t g_caught;

uint32_t add_one(uint32_t x) {
    try {
        if (x == 42u) {
            throw BadInput{};
        }
    } catch (const BadInput&) {
        // Observable but irrelevant to the return value.
        g_caught = x;
    }
    return x + 1;
}
