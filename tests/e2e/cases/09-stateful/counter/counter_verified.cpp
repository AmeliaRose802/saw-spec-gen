// counter — a *wide typed field* stateful post-state. The Counter
// object is a single `uint32_t` accessed as one i32 load/store, which
// the byte-array object model cannot satisfy (an i32 read against a
// `[4 x i8]` allocation fails in SAW's memory model). The typed
// out-buffer shape `i32` allocates the object as `llvm_int 32` so the
// wide access type-checks:
//
//   --out-buffer-param c=i32  --cryptol-fn-out c=counter_inc_post
//
// verified : the field is incremented by one. The Cryptol model adds
//            one to the 32-bit pre-state; SAW proves equality (the add
//            wraps mod 2^32, matching unsigned C++ overflow semantics).

#include <cstdint>

struct Counter {
    std::uint32_t n;
};

std::uint32_t counter_inc(Counter* c) noexcept {
    c->n = c->n + 1;
    return 1;
}
