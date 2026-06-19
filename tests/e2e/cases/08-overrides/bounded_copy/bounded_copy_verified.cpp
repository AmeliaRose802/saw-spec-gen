// bounded_copy — exercises saw-spec-gen's `--out-buffer-param` /
// `--in-buffer-size` / `--cryptol-fn-out` override branch.
//
// Copies the first `nb` bytes of `in` into `out`, leaving bytes at
// positions [nb, 4) untouched (i.e. equal to their pre-call value).
// Returns the number of bytes written = nb.
//
// The output-buffer postcondition can't be captured by `--cryptol-fn`
// alone (that only models the return value).  Instead it's expressed
// via `--cryptol-fn-out out=bounded_copy_post`, which uses the
// pre-state of `out` (read by `out_pre`) as one of its arguments —
// the same shape demo_protocol's canonicalize_lp uses in production.

#include <cstddef>
#include <cstdint>

std::size_t bounded_copy(std::uint8_t* out,
                         const std::uint8_t* src,
                         std::uint8_t nb) noexcept {
    for (std::uint8_t i = 0; i < nb; ++i) {
        out[i] = src[i];
    }
    return nb;
}
