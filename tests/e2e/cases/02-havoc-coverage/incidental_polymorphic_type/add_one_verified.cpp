#include <cstdint>

/*
DEMO / REGRESSION (issue #57): the target `add_one` is a plain,
non-virtual function. The translation unit *incidentally* declares a
polymorphic class hierarchy (`ILogger` with a pure-virtual `log`, plus a
concrete `FileLogger` override), but `add_one` never constructs a logger
and never dispatches through the vtable.

Before the fix, saw-spec-gen would still extract every virtual method in
the TU and emit `vtable_stubs.ll` + `interface_overrides.saw`, forcing a
combined-module load even though none of that machinery is reachable
from `add_one`. When a virtual method's class could not be resolved the
stub degenerated to an `unknown_*` symbol, and the generated script
failed to run without manual patching.

The reachability gate now recognizes that `add_one` cannot reach any
virtual method and skips interface emission entirely, so the generated
script uses the single-module load path and verifies cleanly.
*/

// Polymorphic hierarchy that exists in the TU but is unreachable from
// the target function below.
class ILogger {
public:
    virtual ~ILogger() = default;
    virtual void log(const char* message) = 0;
    virtual int level() const = 0;
};

class FileLogger : public ILogger {
public:
    void log(const char* message) override { (void)message; }
    int level() const override { return 3; }
};

// Spec: add_one(x) = x + 1. No polymorphism touched.
uint32_t add_one(uint32_t x) {
    return x + 1;
}
