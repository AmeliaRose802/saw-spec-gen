#include <cstdint>
#include <cstdio>
#include <cstdlib>

int super_important = 7;

/*
DEMO: VERIFIED — OkLog::log has a concrete body in-module, so gen-verify
skips the havoc override and SAW verifies the real implementation.

The ILog interface has a non-const log() method — through vtable
dispatch, SAW would have to assume it could clobber globals, class
members, or pointer arguments (havoc model).

This case instantiates OkLog directly and calls log() on the concrete
type.  Because OkLog::log() has a visible body in the same translation
unit, gen-verify does NOT emit a havoc override for it.  SAW
symbolically executes the real implementation, proves it never touches
super_important, and therefore the branch is dead — result: VERIFIED.

COMPARE: add_one_disproved.cpp uses SusLog, whose log() DOES clobber
super_important, so that version is correctly DISPROVED.
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
    // Concrete type — SAW verifies OkLog::log() directly,
    // not the havoc model of ILog::log()
    OkLog logger;

    logger.log("Adding one to x");

    // SAW verified that OkLog::log() doesn't touch super_important,
    // so this branch is provably dead
    if (super_important == -1) {
        return 12;
    }

    return x + 1;
}
