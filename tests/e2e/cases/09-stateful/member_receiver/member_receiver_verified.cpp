// member_receiver — exercises inferred receiver-backed state for a
// non-static C++ method without an explicit `--out-buffer-param this=...`
// CLI override. The receiver object is a single 64-bit word so the
// inferred `this=auto` layout matches the generated `[64]` model.

#include <cstdint>

struct KeyStore {
    std::uint64_t state;

    std::uint8_t activate() noexcept;
};

std::uint8_t KeyStore::activate() noexcept {
    state = 0x1122334455667788ULL;
    return 1;
}
