// SPDX-License-Identifier: GPL-2.0-or-later
#pragma once
#include <atomic>
#include <chrono>
#include <cstdint>
#include <string_view>

namespace SonantePcm {
// Only this Windows transport preserves its FIFO on pause. Explicit Cancel
// retains its terminal LOST semantics; no counter/claim/EOF is reset.
constexpr bool PreservePause(std::string_view plugin) noexcept {
    return plugin == "sonante_private_pcm";
}
class PauseClock {
    std::atomic_bool paused{false};
public:
    bool Pause(std::uint32_t flags) noexcept {
        // EOF may be set by the parent to drain a held generation. Pausing it
        // does not reopen it or admit another frame; Play still requires zero.
        if (flags & ~1u) return false;
        paused.store(true);return true;
    }
    bool Play(std::uint32_t flags) noexcept {
        if (flags != 0) return false;
        paused.store(false);return true;
    }
    void Close() noexcept { paused.store(false); }
    std::chrono::steady_clock::duration Delay() const noexcept {
        return paused.load() ? std::chrono::steady_clock::duration::max()
                             : std::chrono::steady_clock::duration::zero();
    }
};
}
