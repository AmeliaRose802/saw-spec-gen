#include <cstdint>
#include <cstdio>
#include <cstdlib>

/*
DEMO: Verified because const method cannot modify class member data

Same structure as add_one_disproved.cpp, but prepare() is now const.
A const method guarantees it will not modify `this`, so SAW knows
that `bias_` is still 0 after the call, and the proof succeeds.
*/

class ICounter {
public:
    uint32_t bias_ = 0;

    virtual void prepare(uint32_t x) const = 0;
};

class OkCounter : public ICounter {
public:
    void prepare(uint32_t x) const override {
        printf("Preparing %u\n", x);
    }
};

class BadCounter : public ICounter {
public:
    void prepare(uint32_t x) const override {
        // Can't do: bias_ = 42;  — compiler error, method is const
        printf("Bad but harmless %u\n", x);
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

    // const method — SAW knows bias_ is unchanged (still 0)
    counter->prepare(x);

    // bias_ is guaranteed to be 0, so this is x + 1
    return x + 1 + counter->bias_;
}
