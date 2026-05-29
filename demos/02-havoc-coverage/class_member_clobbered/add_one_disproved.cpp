#include <cstdint>
#include <cstdio>
#include <cstdlib>

/*
DEMO: Disproved due to class member data being modified

A virtual method call can modify the object's own member data.
Since SAW models virtual dispatch conservatively (havoc),
any non-const method may modify `this`, making subsequent
reads of class members unpredictable.

The `prepare()` method is non-const, so SAW must assume it
could modify `bias_`. After the call, `bias_` is unconstrained
and the return value `x + 1 + bias_` != `x + 1`.

FIX: Make the method const (see add_one_verified.cpp).
*/

class ICounter {
public:
    uint32_t bias_ = 0;

    virtual void prepare(uint32_t x) = 0;
};

class OkCounter : public ICounter {
public:
    void prepare(uint32_t x) override {
        // Doesn't modify bias_ — safe in practice
        printf("Preparing %u\n", x);
    }
};

class BadCounter : public ICounter {
public:
    void prepare(uint32_t x) override {
        // Clobbers class member data!
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

    // Non-const method
    counter->prepare(x);

    // If prepare() modified bias_, this returns x + 1 + 42, not x + 1
    return x + 1 + counter->bias_;
}
