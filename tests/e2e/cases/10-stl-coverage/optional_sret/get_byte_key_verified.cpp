/*
VERIFIED: std::optional<proto::ByteKey> return type via sret.

proto::ByteKey uses uint8_t data[16] (alignment == 1) so that
std::optional<ByteKey> is exactly 17 bytes with NO trailing padding.
17 > 16 bytes forces the sret calling convention on x86-64.

All three pipeline fixes are exercised:

  Fix 1 (alias_fallbacks_ir.rs — sret byte-size fallback):
    Clang omits dereferenceable(N) on output-only sret slots.
    The fallback reads the byte size (17) from the struct's TypeInfo.

  Fix 2 (spec_rewrite.rs — is_complex_stl_template):
    std::optional<T> is rewritten to llvm_array 17 (llvm_int 8)
    on MSVC ABI so SAW does not abort with "unsupported type".

  Fix 3 (type_resolve.rs — normalize_template_args):
    Itanium ABI IR emits class.std::optional<proto::ByteKey> while
    the AST qualType stores std::optional<ByteKey>.
    normalize_template_args strips "proto::" and the names match.

Output layout (17 bytes, no padding):
  byte  0    : id  (the uint8_t argument)
  bytes 1-15 : 0   (zero-initialized remaining data fields)
  byte  16   : 0x01 (has_value = true)

Expected: VERIFIED.
*/
#include <cstdint>
#include <optional>

namespace proto {
    // 16 bytes, alignment 1.
    // sizeof(std::optional<ByteKey>) == 17 (no padding) and > 16
    // so it is returned via a hidden sret pointer on x86-64.
    struct ByteKey {
        uint8_t data[16];
    };
}  // namespace proto

// Returns std::optional<proto::ByteKey> via sret.
// data[0] = id, all other data bytes = 0, has_value = 1.
std::optional<proto::ByteKey> get_byte_key(uint8_t id) {
    proto::ByteKey k{};
    k.data[0] = id;
    return k;
}
