#pragma once
#include <cstdint>

struct Payload {
    uint8_t *data;
    uint32_t len;
};

struct Context {
    uint32_t  flags;
    Payload  *payload;   // pointer to heap-allocated Payload
    uint32_t  counter;
};

// Non-const: can modify ctx and everything reachable from it
// (ctx->flags, ctx->counter, ctx->payload, ctx->payload->data, ctx->payload->len)
extern "C" int process(Context *ctx);

// Reads ctx->counter AFTER process() may have trashed it
extern "C" uint32_t get_counter_after_process(Context *ctx) {
    process(ctx);             // process can corrupt ANYTHING in ctx
    return ctx->counter;      // is this still meaningful? Only if we account for havoc
}

// Reads ctx->flags BEFORE process, returns it
// Should be safe even if process trashes ctx, because we read first
extern "C" uint32_t get_flags_then_process(Context *ctx) {
    uint32_t saved = ctx->flags;   // save before the call
    process(ctx);                   // process can trash ctx
    return saved;                   // return the saved copy, not ctx->flags
}
