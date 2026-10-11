# Sonante

**A library-first music player for Linux — local collections and Plex, in one place.**

Sonante is a desktop music player built for people who want to **listen to, navigate, and maintain their music collections** without giving up control of playback. It combines local folders and Plex Media Server with a dedicated [Music Player Daemon (MPD)](https://www.musicpd.org/) instance, a native Rust backend, and a React interface.

**Platform:** Linux (x86_64) · **Interface:** English / Português (Brasil) · **Stack:** React, TypeScript, Tauri, Rust, MPD, ALSA

[Releases](https://github.com/tiagoalam/Sonante/releases) · [Report an issue](https://github.com/tiagoalam/Sonante/issues) · [Source code](https://github.com/tiagoalam/Sonante)

## Highlights

| Area | What Sonante offers |
| --- | --- |
| **Local music** | Multiple local folders and mounted shares; album, artist, year, genre, and folder browsing; source selection and incremental library updates. |
| **Plex** | Account authorization, server/library selection, album and artist navigation, years, genres, collections, and remote playback. |
| **Playback** | Queue management, play/pause, next/previous, seek, session restoration without automatic playback, and desktop media-key integration through MPRIS2. |
| **Collection tools** | Local and Plex favorites, named playlists, availability-aware playlist playback, and an individual-track FLAC tag editor. |
| **Artwork & browsing** | Local and Plex album artwork, virtualized grids for large catalogs, and artwork caching with bounded work. |
| **Audio controls** | Shared and Direct output choices, device availability checks, ReplayGain settings, an explanatory Signal Path, and a separate Now Playing window with visualization. |

Sonante is designed for substantial, imperfect real-world libraries: multiple storage locations, multi-disc albums, CUE-based albums, varying metadata, and sources that may temporarily become unavailable.

## Installation

Download Sonante v0.4.9 from [GitHub Releases](https://github.com/tiagoalam/Sonante/releases/tag/v0.4.9). Prebuilt packages are available for **Debian/Ubuntu (DEB), Fedora (RPM), and Arch Linux/Manjaro (`.pkg.tar.zst`)**, for x86_64. Choose the package for your distribution.

### Debian / Ubuntu — DEB

Download the `.deb` package and `SHA256SUMS-ubuntu.txt` from the release page, then run from their directory:

```bash
sha256sum --ignore-missing -c SHA256SUMS-ubuntu.txt
sudo apt install ./Sonante_0.4.9_amd64.deb
```

### Fedora — RPM

Download the `.rpm` package and `SHA256SUMS-ubuntu.txt` from the release page:

```bash
sha256sum --ignore-missing -c SHA256SUMS-ubuntu.txt
sudo dnf install ./Sonante-0.4.9-1.x86_64.rpm
```

The required MPD and native libraries must be available from your configured repositories. Sonante does not add third-party repositories.

### Arch Linux / Manjaro — pacman

Download the `.pkg.tar.zst` package and `SHA256SUMS-arch.txt` from the release page:

```bash
sha256sum -c SHA256SUMS-arch.txt
sudo pacman -U ./sonante-0.4.9-1-x86_64.pkg.tar.zst
```

Package installation does **not** enable a system-wide MPD service. Sonante manages a private MPD process for its own session.

### System requirements

- A Linux desktop with **GTK 3 and WebKitGTK 4.1** support.
- **MPD** for playback, decoding, and local-library indexing; available formats depend on the installed MPD build.
- **ALSA** and `aplay` (usually from `alsa-utils`) for audio devices.
- **OpenSSL 3**, `xdg-open`, and an **XDG Desktop Portal** backend for integration with the desktop and folder selection.
- **WirePlumber / `wpctl` is optional**, used when available for Shared-mode volume control through PipeWire.
- **`webp-pixbuf-loader` is optional for artwork**: it is recommended by the DEB/RPM packages and listed as an optional dependency in Arch. Without it, some WebP artwork previews may be unavailable.

Exact native dependencies vary by distribution and are checked as part of packaging. The Ubuntu 22.04 build is additionally checked against a maximum required **GLIBC 2.35** symbol version; this does not guarantee compatibility with every distribution or device.

## Getting started

1. **Choose an audio output.** Shared mode uses the system's default ALSA route; Direct mode targets a selected ALSA hardware endpoint.
2. **Add your music.** Register one or more local folders, connect a Plex server, or use both.
3. **Browse your collection.** Explore albums, artists, years, genres, and the source-specific views; add tracks to the queue or save them to a playlist.
4. **Adjust playback.** Use the player controls, your desktop media keys, or the Now Playing window.

The first-run wizard guides you through these choices. Plex sign-in uses an external authorization flow rather than asking you to paste a token into the interface.

## Library management

### Local collections

Sonante presents multiple configured roots through a managed virtual library indexed by MPD. Library changes use guarded preparation and verification rather than rewriting the original music files simply to make them discoverable. Mounted volumes and network shares may be used as local sources if they are accessible to the process.

Local browsing includes **Albums, Artists, Years, Genres, and Folders**. Album handling also accounts for multi-disc layouts and supported physical/virtual CUE representations. The interface is designed to remain usable with large catalogs through pagination and virtualized rendering.

**Tag editing:** the current editor can inspect supported local-track metadata and edit **individual FLAC files** using a temporary-file-and-replace approach with conflict checks. Editing metadata intentionally changes the selected physical file. Plex tracks, virtual CUE tracks, and unsupported formats are not writable through this editor. **Batch album tag editing is not part of v0.4.9.**

### Plex libraries

Plex connectivity is associated with a server's stable identity, not just a temporary URL. Sonante can use validated server connections and resolve playback references when needed. It offers **Albums, Artists, Years, Genres, and Collections**, along with favorites and playlists spanning supported local and Plex items.

Artwork requests are handled through the backend; authenticated Plex URLs and access tokens are not exposed as frontend image URLs. A missing or unreachable Plex source is reported as unavailable rather than being treated as a permanently empty collection.

## Audio: what Shared and Direct mean

Sonante uses its **own supervised MPD process** as the actual decoder and playback engine. The application itself does not implement a separate PCM decoder/output pipeline.

| | Shared | Direct |
| --- | --- | --- |
| **MPD output** | ALSA `default` | Selected ALSA hardware endpoint (typically `hw:CARD=...,DEV=...`) |
| **System audio** | May coexist with other applications through PipeWire, PulseAudio, or dmix, depending on the host | Requests access to a hardware endpoint; availability and exclusivity depend on the device and host |
| **Volume** | PipeWire sink via `wpctl` when available and verified; otherwise MPD software volume | ALSA hardware mixer when a usable control can be identified; otherwise MPD software volume |
| **Resampling / mixing** | May occur in the host audio stack | Depends on the effective MPD/ALSA/device configuration |

The interface offers **ReplayGain** controls and can request **DoP** when configured for a compatible Direct output. DoP is not the same as native DSD, and enabling it does not prove that a DAC received DSD data.

**Sonante does not claim verified bit-perfect output.** The displayed sample rate, bit depth, and channel information are **reported by MPD**, not measured at the DAC. Software volume, ReplayGain, resampling, and external mixing can change the actual signal. The Signal Path view is an explanation of the selected route and known status, **not an end-to-end audio measurement**.

For actual decoder support on your system, inspect `mpd --version`.

## Privacy and application data

Sonante stores its per-user configuration and library state locally. Typical paths (under the XDG configuration directory) include:

| File / directory | Purpose |
| --- | --- |
| `~/.config/sonante/config.json` | Audio, source, and Plex settings |
| `~/.config/sonante/favorites.json` | Favorite albums |
| `~/.config/sonante/queue_cache.json` | Cached queue metadata |
| `~/.config/sonante/mpd.conf` and `mpd.db` | Private MPD configuration and catalog |
| `~/.config/sonante/library/` | Managed virtual library |
| `$XDG_RUNTIME_DIR/sonante/` | Private MPD runtime socket and process metadata, when the runtime directory is available |

Configuration directories and sensitive files are created with restrictive Linux permissions. Plex credentials require special handling: **do not share configuration files or diagnostic output containing tokens**. Uninstalling the application package does not itself remove your personal library or configuration files.

## Build from source

Development requires **Node.js 22**, npm, a stable Rust toolchain, and the native development dependencies for Tauri 2, GTK 3, WebKitGTK 4.1, ALSA, and OpenSSL. See the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/) and the [CI workflow](https://github.com/tiagoalam/Sonante/blob/main/.github/workflows/ci.yml) for the Ubuntu build environment.

To build **v0.4.9** from its release tag without installing a system package:

```bash
git clone https://github.com/tiagoalam/Sonante.git
cd Sonante
git checkout v0.4.9
npm ci
npm run tauri -- build --no-bundle -- --locked
```

The native executable is written to `src-tauri/target/release/sonante`.

For development:

```bash
npm run tauri dev
```

For tests and checks:

```bash
npm run test:frontend
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo check --manifest-path src-tauri/Cargo.toml --locked
python3 -m unittest discover -s tests -p 'packaging_test.py'
```

The CI also checks packaging versions, native ELF requirements, and package contents. The Arch package is built using an Arch environment; **AppImage is not a validated distribution target for v0.4.9**.

## Project status and scope

Sonante prioritizes **reliable playback, conservative MPD lifecycle management, safe library changes, and clear audio reporting** over claiming support for every possible codec or audio feature. Platform and hardware behavior must be tested rather than inferred from a successful build.

The current local tag editor handles single FLAC tracks, not batch album edits; availability of codecs, network sources, and DAC capabilities depends on your system. Sonante v0.4.9 provides DEB, RPM, and Arch packages for x86_64. Compatibility depends on the distribution, its available dependencies, and the selected audio hardware.

Bug reports and reproducible feedback can be submitted through [GitHub Issues](https://github.com/tiagoalam/Sonante/issues).

## License

**Copyright © 2026 Tiago Alam.**

Sonante's original code is licensed under the **GNU General Public License,
version 3 only** (`GPL-3.0-only`). You may use, modify, and redistribute it under
the terms of [LICENSE](LICENSE). No later version is automatically permitted.
Sonante is provided without any warranty.

See [AUTHORS](AUTHORS) for authorship. Third-party libraries and assets retain
their respective licenses; see [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
The Lucide elements used in the application icon and UI retain their ISC/MIT
notices. Distribution of compiled packages also requires making the corresponding
source available under the applicable licenses, including the sources of bundled
dependencies.
