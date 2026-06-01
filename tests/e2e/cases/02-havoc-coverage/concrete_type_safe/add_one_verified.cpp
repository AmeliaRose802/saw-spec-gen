#include <cstdint>
#include <cstdio>
#include <cstdlib>

int super_important = 7;

/*
DEMO: DISPROVED — even though OkLog::log has a concrete body in-module,
it calls printf (an external system function). The adversarial override
for printf conservatively havocs all mutable globals (since e.g. %n can
write through pointer args), so SAW finds a counterexample where
super_important is clobbered to -1 after the printf call.

COMPARE: add_one_disproved.cpp uses SusLog, whose log() genuinely
clobbers super_important — same DISPROVED verdict, different mechanism.
*/

class ILog {
public:
    virtual void log(const char* message) = 0;
};

class OkLog : public ILog {
public:
    // NOT const — but the implementation is safe
    void log(const char* message) override {
        printf("OK: %s\n", message);
    }
};

class SusLog : public ILog {
public:
    // NOT const — and the implementation IS dangerous
    void log(const char* message) override {
        super_important = -1;  // clobbers global!
        printf("SUS: %s\n", message);
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    // Concrete type — SAW executes OkLog::log() directly,
    // but printf inside it gets a havoc override that clobbers globals
    OkLog logger;

    logger.log("Adding one to x");

    // printf's havoc override may clobber super_important,
    // so SAW cannot prove this branch is dead → DISPROVED
    if (super_important == -1) {
        return 12;
    }

    return x + 1;
}
