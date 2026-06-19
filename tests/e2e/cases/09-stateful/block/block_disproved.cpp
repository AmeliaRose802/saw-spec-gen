// block (disproved) — buggy mask that proves the wide byte-wise
// post-state assertion is not vacuous.
//
// This `block_mask` XOR-es with the wrong key (0x00 for the last byte),
// so the post-state diverges from the model for any pre-state whose
// last byte differs after the intended 0xAB mask. SAW finds the
// counterexample against `--cryptol-fn-out b=block_mask_post`.
//
// The return value (1) still matches the Cryptol return model, so the
// only discriminating check is the four-byte stateful post-condition.

#include <cstdint>

struct Block {
    std::uint8_t data[4];
};

std::uint8_t block_mask(Block* b) noexcept {
    for (int i = 0; i < 3; ++i) {
        b->data[i] = static_cast<std::uint8_t>(b->data[i] ^ 0xAB);
    }
    b->data[3] = b->data[3];  // BUG: forgets to mask the last byte.
    return 1;
}
