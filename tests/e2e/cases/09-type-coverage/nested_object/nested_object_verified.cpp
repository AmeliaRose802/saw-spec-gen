#include <cstdint>

// A plain nested aggregate: `Outer` embeds `Inner` by value, so the
// second field `inner.b` lives at byte offset 4 and the compiler emits a
// natural-aligned load (`load i32, ptr %p, align 4`).
//
// saw-spec-gen models the `const Outer*` parameter as a flat
// `llvm_array 8 (llvm_int 8)` byte buffer. SAW allocates such buffers at
// 1-byte alignment by default, which makes the align-4 sub-object load
// fail with a vacuous `Error during memory load` — masking any real
// verdict. The `object_allocator` helper pins byte-array buffers to the
// platform's fundamental alignment (8) so the aligned read succeeds and
// the proof reaches a genuine verdict.
struct Inner {
    std::uint32_t a;
    std::uint32_t b;
};

struct Outer {
    Inner inner;
};

std::uint32_t read_nested(const Outer* o) noexcept { return o->inner.b; }
