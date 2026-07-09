// enrollment — a *heterogeneous mixed-width struct* post-state with
// alignment padding. The object carries a byte-granular `engaged` flag
// followed by an 8-byte `createdAt` word, so LLVM inserts 7 bytes of
// padding between them. A flat byte array fails the i64 store, and a
// homogeneous `iW`/`NxiW` shape can't also model the byte field.
//
// `--out-buffer-param k=struct:EnrollmentKey` emits
// `llvm_alloc (llvm_struct "struct.EnrollmentKey")`, so SAW uses the
// loaded LLVM layout: typed i8/i64 fields with implicit padding bytes
// left unconstrained. The Cryptol model sees only the typed fields as
// the tuple ([8],[64]):
//
//   --out-buffer-param k=struct:EnrollmentKey
//   --cryptol-fn-out   k=enroll_key_post
//
// verified : `createdAt` is overwritten with a fixed i64 constant and
//            `engaged` is preserved.

#include <cstdint>

struct EnrollmentKey {
    std::uint8_t engaged;
    std::int64_t createdAt;
};

std::uint32_t enroll_key(EnrollmentKey* k) noexcept {
    std::uint8_t engaged = k->engaged;
    k->createdAt = 42;
    k->engaged = engaged;
    return static_cast<std::uint32_t>(engaged);
}
