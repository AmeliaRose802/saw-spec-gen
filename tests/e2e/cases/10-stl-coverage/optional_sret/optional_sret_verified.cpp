/*
 * E2E regression: std::optional<T> return type alias / layout resolution.
 *
 * get_key() returns std::optional<proto::Key> via sret (the type is too
 * large to fit in a register, so clang emits a hidden out-pointer).
 * Three pipeline fixes land together to make this verifiable:
 *
 *   Fix 1 (alias_fallbacks_ir.rs):
 *     Clang omits dereferenceable(N) on output-only sret slots. The
 *     fallback now reads the byte size from the struct's TypeInfo layout.
 *
 *   Fix 2 (spec_rewrite.rs — is_complex_stl_template):
 *     std::optional<T> appears in the IR symbol table (MSVC ABI) but SAW
 *     aborts with "unsupported type: std::optional<proto::Key>" even after
 *     alias resolution. is_complex_stl_template() forces these types to
 *     llvm_array N (llvm_int 8) regardless of IR presence.
 *
 *   Fix 3 (type_resolve.rs — normalize_template_args):
 *     Itanium ABI emits class.std::optional<proto::Key> in the IR symbol
 *     table while the AST qualType stores std::optional<Key> (no namespace).
 *     normalize_template_args strips each template argument to its last ::
 *     component so the two names match.
 *
 * Expected: gen-verify succeeds and the generated verify.saw uses
 * llvm_array N (llvm_int 8) for the return type, not
 * llvm_alias "std::optional<proto::Key>".
 */
#include <cstdint>
#include <optional>

namespace proto {
    struct Key {
        uint32_t id;
        uint32_t flags;
    };
}  // namespace proto

// Returns std::optional<proto::Key> via sret.
std::optional<proto::Key> get_key(uint32_t id) {
    if (id == 0) {
        return std::nullopt;
    }
    return proto::Key{id, 1u};
}
