#include "deep_havoc.h"

// Force the compiler to emit the class definitions
void force_emit(const IProcessor *p, Config *cfg) {
    p->check(cfg);
}

// UNSAFE: reads cfg->flags AFTER non-const process() could clobber it.
// The adversarial havoc spec for process() lets the solver choose ANY
// value for cfg->flags after the call, so we can't prove the return
// equals the original.
uint32_t process_and_read_flags(IProcessor *proc, Config *cfg) noexcept {
    proc->process(cfg);       // vtable dispatch → process can trash cfg
    return cfg->flags;        // reads AFTER havoc — unsound!
}

// SAFE: saves cfg->flags into a local BEFORE calling process().
// Even though process() can clobber cfg, the local is on the stack
// and not reachable through cfg, so it survives.
uint32_t save_flags_then_process(IProcessor *proc, Config *cfg) noexcept {
    uint32_t saved = cfg->flags;   // save before the call
    proc->process(cfg);             // vtable dispatch → cfg trashed
    return saved;                   // return the local copy
}
