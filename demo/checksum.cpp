#include "checksum.h"

int validate_packet(const PacketHeader *header) noexcept {
    if (!header)
        return -1;
    if (header->version == 0 || header->version > 3)
        return 1;
    if (header->payload_len > 65536)
        return 2;
    return 0;
}

uint32_t compute_checksum(const uint8_t *data, size_t len) noexcept {
    uint32_t sum = 0;
    for (size_t i = 0; i < len; ++i)
        sum += data[i];
    return sum;
}

void copy_payload(uint8_t *dst, const uint8_t *src, size_t len) noexcept {
    for (size_t i = 0; i < len; ++i)
        dst[i] = src[i];
}
