# Sonante 0.4.0 — draft release notes

Sonante 0.4.0 focuses on predictable MPD supervision, explicit audio state, secure Plex references, and safer local persistence.

## Highlights

- Hardened the MPD process lifecycle with owned-process checks, private per-user runtime files, explicit shutdown behavior, and safer handling of stale sockets and PID files.
- Hardened MPD protocol handling: command arguments are validated and escaped, `ACK`, timeout, EOF, transport failure, and partial responses are distinguished, and compound queue/output operations publish state only after MPD accepts them.
- Added explicit backend-health states so process exit, unavailable sockets, protocol failure, startup failure, and stopping are not presented as valid playback state.
- Added transactional output switching with state capture and rollback behavior for queue, selected track, position, and playback state.
- Added persistent Plex server identity and dynamic resolution across validated local, remote-direct, and relay connections.
- Replaced persisted and frontend Plex playback URLs with stable media references resolved in the backend at playback time.
- Replaced authenticated Plex artwork URLs with stable artwork references. Tokens remain in the backend; legacy favorites and queue artwork are sanitized during migration.
- Added lazy Plex artwork loading, bounded request concurrency, in-memory LRU caching, request deduplication, Blob URL lifecycle handling, and a 600×600 fallback for original artwork above the 8 MiB limit.
- Improved Plex collection loading and collection artwork mosaics while keeping the maximum of four covers.
- Added Shared volume integration through `wpctl` when PipeWire is confirmed. Shared MPD output uses `mixer_type "none"` in that case and otherwise uses the explicit MPD software fallback.
- Added conservative ALSA hardware-mixer detection for Direct output. Direct falls back to MPD software volume when no single safe playback control can be selected.
- Exposed the effective volume backend in the UI as ALSA Hardware, PipeWire, Software, or unavailable without changing volume scaling between backends.
- Restricted Sonante-owned configuration directories to mode `0700` and persisted JSON/atomic temporary files to mode `0600` on Linux, including migration of legacy permissive files.
- Removed unverified fidelity claims. Direct now means access to the selected ALSA hardware endpoint; DoP configuration and MPD-reported formats are not presented as proof of native DSD or an unmodified signal path.

## Distribution

- Arch Linux/Manjaro: `.pkg.tar.zst` package.
- Source: GitHub-generated `.zip` and `.tar.gz` archives for tag `v0.4.0`.
- No `.deb` in this release; a future build will use an Ubuntu 22.04 baseline and clean Ubuntu/Debian VM validation.
- No AppImage in this release because MPD delivery for clean installations has not yet been validated.

## Validation scope

Automated Rust and TypeScript checks cover the protocol, process lifecycle, rollback behavior, volume-backend selection, Plex connection/reference handling, persistence permissions, artwork validation/cache behavior, and migrations. Real DAC behavior, distro-clean installation, and end-to-end audio-path properties still require hardware and VM validation.
