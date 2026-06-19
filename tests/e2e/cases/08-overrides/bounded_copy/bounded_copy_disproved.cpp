// bounded_copy_disproved — buggy variant: always writes 4 bytes,
// zero-padding past `nb`.  This breaks the postcondition modelled by
// `bounded_copy_post` (which says bytes past nb stay equal to their
// pre-state) and should make SAW return DISPROVED.
//
// Purpose: prove the override-branch postcondition isn't vacuous —
// it actually rules out an incorrect implementation.

#include <cstddef>
#include <cstdint>

std::size_t bounded_copy(std::uint8_t* out,
                         const std::uint8_t* src,
                         std::uint8_t nb) noexcept {
    for (std::uint8_t i = 0; i < 4; ++i) {
        out[i] = (i < nb) ? src[i] : 0;
    }
    return nb;
}
