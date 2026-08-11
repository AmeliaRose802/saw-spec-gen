/*
E2E regression: std::optional<T> return type alias / layout resolution.

get_key() returns std::optional<proto::Key> via a hidden sret pointer
(the struct is 16 bytes so std::optional<proto::Key> is 20 bytes —
above the 16-byte register-return threshold on x86-64 Itanium ABI).
Three pipeline fixes land together to make this reachable by SAW:

  Fix 1 (alias_fallbacks_ir.rs — sret struct size fallback):
    Clang omits dereferenceable(N) on output-only sret slots. The
    fallback now reads the byte size from the struct's TypeInfo layout.
    is_sret_param() previously checked for the "sret" annotation that
    extract_sret() had already stripped; it now also checks WriteOnly.

  Fix 2 (spec_rewrite.rs — is_complex_stl_template):
    std::optional<T> appears in the IR symbol table (MSVC ABI) but SAW
    aborts with "unsupported type" even after alias resolution.
    is_complex_stl_template() forces these types to
    llvm_array N (llvm_int 8) regardless of IR presence.

  Fix 3 (type_resolve.rs — normalize_template_args):
    Itanium ABI emits class.std::optional<proto::Key> in the IR symbol
    table while the AST qualType stores std::optional<Key> (no namespace).
    normalize_template_args strips each template argument to its last ::
    component so the two names match.

The Cryptol spec deliberately returns all-zero bytes, which is wrong:
get_key always writes non-zero data (id, flags, has_value=1), so SAW
executes the function symbolically and finds a counterexample.

The implementation always returns a value (no nullopt branch) to keep
the -O0 IR straightforward: a single constructor call chain, no
invoke/unwind paths in the target function body.

Expected: DISPROVED.
*/
#include <cstdint>
#include <optional>

namespace proto {
    // 16 bytes: large enough that std::optional<Key> (~20 bytes) is
    // returned via a hidden sret pointer on x86-64 (threshold is 16 bytes).
    struct Key {
        uint32_t id;
        uint32_t flags;
        uint32_t version;
        uint32_t reserved;
    };
}  // namespace proto

// Returns std::optional<proto::Key> via sret.
// Always constructs a value (no nullopt path) so the -O0 IR stays
// simple: one constructor call, no invoke/unwind in the function body.
std::optional<proto::Key> get_key(uint32_t id) {
    return proto::Key{id, 1u, 0u, 0u};
}
