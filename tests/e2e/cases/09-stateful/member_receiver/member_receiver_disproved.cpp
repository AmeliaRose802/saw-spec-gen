// member_receiver — disproved twin for inferred receiver-backed state.

#include <cstdint>

struct KeyStore {
    std::uint64_t state;

    std::uint8_t activate() noexcept;
};

std::uint8_t KeyStore::activate() noexcept {
    state = 0x8877665544332211ULL;
    return 1;
}
