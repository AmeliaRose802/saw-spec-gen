#include <cstdint>
#include <cstdio>
#include <cstdlib>

int super_important = 7;

/*
DEMO: Disproved because the concrete type's method clobbers global memory

Unlike the disproved version which uses OkLog (safe implementation),
this version uses SusLog directly. Even though we're calling on
a concrete type (no vtable havoc), SAW verifies the ACTUAL
implementation — and SusLog::log() modifies super_important.

The proof fails because the branch `super_important == -1` is
reachable and returns 12 instead of x + 1.

COMPARE: add_one_verified.cpp uses OkLog — same non-const method,
but the concrete implementation doesn't touch global state.
*/

class ILog {
public:
    virtual void log(const char* message) = 0;
};

class OkLog : public ILog {
public:
    void log(const char* message) override {
        printf("OK: %s\n", message);
    }
};

class SusLog : public ILog {
public:
    void log(const char* message) override {
        super_important = -1;  // clobbers global!
        printf("SUS: %s\n", message);
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    // Concrete type — SAW verifies SusLog::log() directly,
    // and finds it DOES clobber super_important
    SusLog logger;

    logger.log("Adding one to x");

    // SAW proves this branch IS reachable — returns 12, not x+1
    if (super_important == -1) {
        return 12;
    }

    return x + 1;
}
