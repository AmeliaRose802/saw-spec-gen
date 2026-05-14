#pragma once
#include <cstdint>
#include <cstddef>

struct PacketHeader {
    uint32_t version;
    uint64_t timestamp;
    uint32_t payload_len;
};

// Validate a packet header; returns 0 on success, error code otherwise.
int validate_packet(const PacketHeader *header) noexcept;

// Compute a simple checksum over a byte buffer.
uint32_t compute_checksum(const uint8_t *data, size_t len) noexcept;

// Copy payload bytes from src to dst (dst must have room for len bytes).
void copy_payload(uint8_t *dst, const uint8_t *src, size_t len) noexcept;
