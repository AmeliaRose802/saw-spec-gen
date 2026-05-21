#include <cstdint>
#include <cstdio>
#include <cstdlib>

int super_important = 7;

/*
DEMO: Was SAT under the old (inlining) gen-verify pipeline, now UNSAT under
compositional gen-verify.

The ILog interface has a non-const log() method — through vtable
dispatch, SAW would have to assume it could clobber globals, class
members, or pointer arguments (havoc model).

This case instantiates OkLog directly and calls log() on the concrete
type.  An optimistic verifier could inline OkLog::log() (which only
calls printf) and prove super_important is never touched.  The old
gen-verify did exactly that — no override for OkLog::log, SAW would
symbolically execute it.

gen-verify is now compositional: every function the target calls
directly (virtual OR concrete) gets an adversarial override that
havocs all visible globals.  That keeps verification tractable when
sub-functions are complex (loops, indirect calls, etc.) at the cost
of precision here — the override for OkLog::log is allowed to clobber
super_important, so SAW finds a counterexample where add_one returns
12 instead of x + 1.

To recover precision, replace the auto-generated havoc spec for
OkLog::log with a hand-written one that asserts the global is
preserved.
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
