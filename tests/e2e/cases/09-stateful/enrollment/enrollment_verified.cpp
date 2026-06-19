// enrollment — a *heterogeneous mixed-width struct* post-state. The
// EnrollmentKey object models a `std::optional<uint32_t>`-style record:
// a wide `uint32_t id` payload (accessed as a single i32 load/store)
// next to a byte-granular `engaged` flag. Neither the byte-array nor
// the homogeneous `iW`/`NxiW` out-buffer shapes can describe this
// layout — a `[N x i8]` allocation rejects the i32 access, and a
// single `iW`/`NxiW` can't carry two different field widths.
//
// The struct out-buffer shape allocates the object as
// `llvm_struct_type [ llvm_int 32, llvm_int 8 ]`, so SAW presents the
// pre-state as a Cryptol tuple ([32],[8]) and both the wide and the
// byte field type-check:
//
//   --out-buffer-param k={i32,i8}  --cryptol-fn-out k=enroll_key_post
//
// verified : `id` is incremented by one and the `engaged` flag is
//            preserved. The Cryptol model maps (id, engaged) ->
//            (id + 1, engaged); SAW proves equality (the add wraps mod
//            2^32, matching unsigned C++ overflow semantics).

#include <cstdint>

struct EnrollmentKey {
    std::uint32_t id;
    std::uint8_t engaged;
};

std::uint32_t enroll_key(EnrollmentKey* k) noexcept {
    k->id = k->id + 1;
    return 1;
}
