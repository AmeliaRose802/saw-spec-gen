#pragma once
#include <cstdint>

// A struct with pointer fields — the interesting case for deep havoc
struct Buffer {
    uint8_t *data;
    uint64_t len;
};

struct Config {
    uint32_t flags;
    Buffer   *input_buf;    // pointer to a Buffer (which itself has a pointer)
    Buffer   *output_buf;
};

// Abstract interface with methods that take structs containing pointers
class IProcessor {
public:
    virtual ~IProcessor() = default;

    // const method: must not modify anything reachable from config
    virtual bool check(const Config *cfg) const noexcept = 0;

    // non-const method: can modify config and everything reachable from it
    virtual int process(Config *cfg) noexcept = 0;
};

// --------------------------------------------------------------------------
// Functions under verification. Both call through the IProcessor vtable.
// --------------------------------------------------------------------------

// UNSAFE: reads cfg->flags AFTER non-const process() could clobber it.
// Proof that return == original flags should FAIL.
extern "C" uint32_t
process_and_read_flags(IProcessor *proc, Config *cfg) noexcept;

// SAFE: saves cfg->flags BEFORE calling process(), returns the saved copy.
// Proof that return == original flags should SUCCEED.
extern "C" uint32_t
save_flags_then_process(IProcessor *proc, Config *cfg) noexcept;
