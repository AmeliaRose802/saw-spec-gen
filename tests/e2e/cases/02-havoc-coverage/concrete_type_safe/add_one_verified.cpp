#include <cstdint>

int super_important = 7;

/*
DEMO: VERIFIED — OkLog::log has a concrete body whose entire call-chain
is visible in the bitcode (no opaque externs). The per-target BFS traces
all stores reachable from add_one and confirms none of them touch
super_important, so SAW proves add_one(x) == x + 1.

Key detail: OkLog::log does NOT call printf or any other declare-only
function. If it did, the opaque callee would conservatively clobber all
externally-visible globals (including super_important) because any
function in another TU could `extern int super_important;` and write it.

COMPARE: add_one_disproved.cpp uses SusLog, whose log() directly stores
to super_important — BFS catches that and the override clobbers it.
*/

class ILog {
public:
    virtual void log(const char* message) = 0;
};

// Function-local statics compile to internal-linkage LLVM globals.
// saw-spec-gen discovers these from the IR and emits llvm_alloc_global
// so SAW can symbolically execute bodies that touch them.
static int safe_counter = 0;

class OkLog : public ILog {
public:
    // Body is fully visible — no opaque extern calls.
    // Stores to safe_counter (internal linkage) but never to
    // super_important.
    void log(const char* message) override {
        safe_counter++;
        (void)message;
    }
};

class SusLog : public ILog {
public:
    // NOT const — and the implementation IS dangerous
    void log(const char* message) override {
        super_important = -1;  // clobbers global!
    }
};

// Spec: add_one(x) = x + 1
uint32_t add_one(uint32_t x) {
    // Concrete type — SAW executes OkLog::log() directly.
    // Its body stores to safe_counter (internal linkage, discovered
    // from IR), never to super_important → branch below is dead.
    OkLog logger;

    logger.log("Adding one to x");

    // super_important is preserved — OkLog::log doesn't touch it
    if (super_important == -1) {
        return 12;
    }

    return x + 1;
}
