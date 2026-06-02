#include <cstdint>
#include <cstdio>
#include <cstdlib>

/*
DEMO: Disproved because output buffer is read before being written

The function passes an uninitialized buffer to a virtual method
that is supposed to fill it (_Out_ semantics). Through havoc,
SAW must assume the method may not write to the buffer at all.
The subsequent read of the buffer yields an unconstrained value.

FIX: Use _Out_ SAL annotation so the tool knows the method is
     obligated to write to the buffer, and the initial contents
     don't matter (see add_one_verified.cpp).
*/

class IComputer {
public:
    // No annotation — SAW doesn't know if buf is read, written, or both
    virtual void compute(uint32_t x, uint32_t* buf) = 0;
};

class OkComputer : public IComputer {
public:
    void compute(uint32_t x, uint32_t* buf) override {
        // Writes the result — correct behavior
        *buf = x + 1;
    }
};

class BadComputer : public IComputer {
public:
    void compute(uint32_t x, uint32_t* buf) override {
        // Reads without writing — uses uninitialized data!
        printf("buf contains: %u\n", *buf);
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    IComputer* c;

    if (rand() % 2 == 0) {
        c = new OkComputer();
    } else {
        c = new BadComputer();
    }

    uint32_t result;  // uninitialized

    // Through havoc, SAW cannot guarantee *result is written
    c->compute(x, &result);

    // result is unconstrained — could be anything
    return result;
}
