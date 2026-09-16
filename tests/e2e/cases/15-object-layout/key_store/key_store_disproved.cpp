#include <cstdint>
#include <mutex>
#include <optional>

struct EnrollmentKey {
    std::uint64_t id;
    std::uint64_t version;
    std::uint64_t generation;
    bool isActive;
};

class KeyStore {
    mutable std::mutex mu_;
    std::optional<EnrollmentKey> key_;
public:
    std::optional<EnrollmentKey> provision(EnrollmentKey newKey);
};

std::optional<EnrollmentKey> KeyStore::provision(EnrollmentKey newKey) {
    std::scoped_lock lock(mu_);
    if (key_) return std::nullopt;
    newKey.isActive = false;
    key_ = newKey;
    // Regression: only the stored post-state is corrupted. Return stays correct.
    key_->version += 1;
    return newKey;
}