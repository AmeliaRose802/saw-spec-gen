/*
Regression test — DISPROVED variant.

Same as add_one_verified.cpp but returns x + 2 instead of x + 1, so
the implementation can never match `add_one_spec x = x + 1`.

RESULT: DISPROVED.
*/

#include <cstdint>

struct Token {
    uint32_t a;
    uint32_t b;
    uint32_t c;
    uint32_t d;
    uint32_t e;
};

// External sub-callee: declared only, no definition.
Token get_token(uint32_t id);

uint32_t add_one(uint32_t x) {
    (void)get_token(x);
    return x + 2;
}
