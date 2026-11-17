// SPDX-License-Identifier: GPL-2.0-or-later
// Sonante Windows candidate transport only. No DSP, timers or global object names.
#pragma once
#include <algorithm>
#include <atomic>
#include <bit>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <span>

namespace SonantePcm {
constexpr std::uint64_t MAGIC = 0x31304d43504e4f53;
constexpr std::uint32_t VERSION = 1, CAPACITY = 4096, FRAME_BYTES = 8;
constexpr std::uint32_t RATE = 48000, CHANNELS = 2, S32_LE = 1;
constexpr std::uint32_t EOF_FLAG = 1, LOST = 2, STOP = 4;
constexpr std::uint64_t INDEX_MAX = 0x7fffffffffffffff;
struct Binding { std::uint32_t owner; std::uint64_t nonce0, nonce1; };
struct alignas(64) Header {
    std::uint64_t magic;
    std::uint32_t version, header_bytes, capacity, rate, channels, frame_bytes, format, owner;
    std::uint64_t nonce0, nonce1, reserved;
    alignas(64) std::uint64_t written;
    std::byte pad_written[56];
    alignas(64) std::uint64_t read;
    std::byte pad_read[56];
    alignas(64) std::uint32_t flags;
    std::uint32_t consumer;
    std::byte pad_flags[56];
    alignas(64) std::byte data[CAPACITY * FRAME_BYTES];
};
static_assert(std::endian::native == std::endian::little);
static_assert(std::atomic_ref<std::uint64_t>::is_always_lock_free);
static_assert(std::atomic_ref<std::uint32_t>::is_always_lock_free);
static_assert(offsetof(Header, written) == 64 && offsetof(Header, read) == 128);
static_assert(offsetof(Header, flags) == 192 && offsetof(Header, consumer) == 196 && offsetof(Header, data) == 256);
static_assert(sizeof(Header) == 33024 && alignof(Header) == 64);
template<typename T> inline T Load(T &v) noexcept {
    return std::atomic_ref<T>(v).load(std::memory_order_acquire);
}
template<typename T> inline void Store(T &v, T next) noexcept {
    std::atomic_ref<T>(v).store(next, std::memory_order_release);
}
inline bool Valid(Header &h, Binding b) noexcept {
    return b.owner != 0 && (b.nonce0 | b.nonce1) != 0 &&
        Load(h.magic) == MAGIC && Load(h.version) == VERSION && Load(h.header_bytes) == 256 &&
        Load(h.capacity) == CAPACITY && Load(h.rate) == RATE && Load(h.channels) == CHANNELS &&
        Load(h.frame_bytes) == FRAME_BYTES && Load(h.format) == S32_LE &&
        Load(h.owner) == b.owner && Load(h.nonce0) == b.nonce0 && Load(h.nonce1) == b.nonce1 &&
        Load(h.reserved) == 0 && Load(h.consumer) <= 1 && (Load(h.flags) & ~(EOF_FLAG | LOST | STOP)) == 0;
}
inline void Signal(Header &h, std::uint32_t flag) noexcept {
    std::atomic_ref<std::uint32_t>(h.flags).fetch_or(flag, std::memory_order_acq_rel);
}
// Only the single producer writes written/data; only the single reader writes
// read. Release publishes bytes before written, acquire prevents early reuse.
// 0 means bounded backpressure; negatives are terminal/invalid, never success.
inline std::ptrdiff_t Write(Header &h, Binding b, std::span<const std::byte> src) noexcept {
    if (!Valid(h, b)) return -1;
    const auto flags = Load(h.flags);
    if ((flags & (LOST | STOP)) != 0) return -2;
    if ((flags & EOF_FLAG) != 0) return -3;
    if (src.size() % FRAME_BYTES != 0) return -4;
    const auto read = Load(h.read), written = Load(h.written);
    if (read > written || written > INDEX_MAX || written - read > CAPACITY) return -1;
    const auto frames = std::min<std::uint64_t>({src.size() / FRAME_BYTES, CAPACITY - (written - read), 512});
    if (frames == 0) return 0;
    if (written > INDEX_MAX - frames) return -1;
    const auto bytes = static_cast<std::size_t>(frames * FRAME_BYTES);
    const auto offset = static_cast<std::size_t>(written % CAPACITY) * FRAME_BYTES;
    const auto first = std::min(bytes, sizeof(h.data) - offset);
    std::memcpy(h.data + offset, src.data(), first);
    std::memcpy(h.data, src.data() + first, bytes - first);
    // Cancellation is terminal. The host invalidates the whole generation;
    // never clear/reset counters or retry this channel after LOST/STOP.
    if ((Load(h.flags) & (LOST | STOP)) != 0) return -2;
    Store(h.written, written + frames);
    return static_cast<std::ptrdiff_t>(bytes);
}
}
