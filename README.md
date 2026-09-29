<div align="center">

# Sonante

**Audiophile Bit-Perfect Desktop Audio Player for Linux**

*Direct ALSA bit-perfect routing, flexible PipeWire shared output, native DSD/PCM streaming, and Plex Media Server integration.*

[![Release](https://img.shields.io/github/v/release/tiagoalam/sonante?style=flat-square&color=E5A00D)](https://github.com/tiagoalam/sonante/releases)
[![Platform](https://img.shields.io/badge/Platform-Linux%20(ALSA%20%7C%20PipeWire)-blue?style=flat-square)](#requirements)
[![Backend](https://img.shields.io/badge/Backend-Rust%20%7C%20Tauri-orange?style=flat-square)](#architecture)
[![Frontend](https://img.shields.io/badge/Frontend-React%2018%20%7C%20TypeScript-blueviolet?style=flat-square)](#architecture)
[![Engine](https://img.shields.io/badge/Audio%20Engine-Dedicated%20MPD-white?style=flat-square)](#audiophile-audio-engine)
[![License](https://img.shields.io/badge/License-Freeware%20%2F%20Source--Available-yellow?style=flat-square)](#license)

</div>

---

## Overview

**Sonante** is a desktop music player crafted for high-fidelity audio on Linux, offering the choice between **exclusive bit-perfect hardware playback** and **everyday system-shared audio**:

* **Bit-Perfect Mode (Direct ALSA):** Bypasses all operating system mixers, software volume controls, and resampling layers (PipeWire/PulseAudio) to speak directly to dedicated DAC hardware nodes (`hw:X,Y`).
* **Shared Mode (PipeWire / PulseAudio):** Routes audio through your system's default sound server, allowing music playback to coexist seamlessly with browsers, Discord, games, and system notifications without monopolizing the device.

Sonante unifies offline high-resolution collections (spanning internal disks and external drives) and remote **Plex Media Server** audio libraries under an elegant, responsive dark interface.

---

## What's New in v0.3.5

* **Persistent Queue & Playback State:** Dedicated disk caching (`queue_cache.json`) preserves the active playlist, playhead position, high-resolution artwork, and metadata across application restarts.
* **Unified Track Duration Engine:** Fixed Plex API millisecond-to-second discrepancies and integrated MPD `lsinfo` duration parsing, ensuring exact time displays across albums and queue drawers.
* **Interactive PlayerBar Navigation:** Album covers, track titles, and artist labels in the bottom control bar now act as contextual links directly navigating to discographies and album views.
* **UI Lifecycle & Render Performance:** Component memoization for album cards (`React.memo`), elimination of duplicate initialization calls, and full metadata support in global Plex searches.

---

## Screenshots

<p align="center">
  <img src="docs/screenshots/library-grid.png" width="100%" alt="Library Grid View" />
  <br><em>Browsing Hi-Res / SACD library with real-time filters and responsive album grid.</em>
</p>

<p align="center">
  <img src="docs/screenshots/album-queue.png" width="49%" alt="Album and Queue View" />
  <img src="docs/screenshots/artist-view.png" width="49%" alt="Artist Discography" />
  <br><em>Left: Album tracklist with slide-out Queue Drawer | Right: Full artist discography and top tracks.</em>
</p>

## Key Features

### 🔊 Audio Output Architecture

Sonante v0.3.8 introduces a dedicated dual-mode audio architecture designed to seamlessly accommodate both critical listening and daily desktop workflows:

* **Bit-Perfect Exclusive Mode (Direct ALSA):**
  * Direct communication with physical hardware endpoints (`hw:CARD,DEV`), bypassing OS mixers, sample-rate converters, and DSP layers for pure, uncolored bit-perfect streaming.
  * Native **DSD over PCM (DoP)** support up to DSD128/DSD256 and bit-perfect Hi-Res PCM streaming up to 384 kHz / 32-bit.
  * Graceful socket teardown and strict device descriptor release ensuring DACs are freed immediately when playback stops or the app is closed.

* **Shared System Mode (PipeWire / PulseAudio / ALSA dmix):**
  * Universal routing through the default system audio server using the standard `default` endpoint.
  * Plays concurrently with web browsers, communication tools, games, and desktop notifications without hardware locking or audio device conflicts.
  * Resilient backend implementation requiring no special MPD plugin dependencies, guaranteeing compatibility across all Linux distributions.

### Plex Media Server Integration
* **Official OAuth / PIN Authentication:** Web-based login with polling and secure local token storage.
* **LAN Auto-Discovery & Direct Play:** Automatically detects whether the server is local or remote, prioritizing local network IP routes for maximum throughput.
* **Lossless Direct Streaming:** Direct stream playback of FLAC, ALAC, and DSD tracks without server-side transcoding.
* **Unified Remote Navigation:** Browse Plex Music Libraries, Collections, Artist Discographies, and perform fast instant search with debounced indexing.

### Local Music Management
* **Multi-Directory Aggregation:** Merge arbitrary local folders, internal disks, and external USB drives under a unified symlink structure.
* **Album Deduplication:** Consolidates loose audio files into organized album collections.
* **LRU Caching & Lazy Loading:** Visual artwork is lazily loaded using an `IntersectionObserver` coupled with an in-memory Least-Recently-Used (LRU) cover cache to minimize RAM consumption.

### UI & User Experience
* **Interactive PlayerBar:** Instant navigation back to current artists and albums directly from playback controls.
* **Fully Internationalized (i18n):** Native support for **English (en-US)** and **Portuguese (pt-BR)** with real-time switching across the entire UI.
* **Interactive First-Run Wizard:** Guides the user through audio output selection (ALSA Bit-Perfect vs PipeWire Shared), local library setup, and Plex connection.
* **Unified Favorites:** Persistent favorites system across both local albums and Plex libraries with active offline availability tracking.
* **Global Keyboard Shortcuts:** Fast control for common playback, volume, and search actions.

---

## Architecture

```text
+-----------------------------------------------------------------+
|                   Frontend (React 18 + Vite)                    |
|      Lucide Icons  *  Tailwind CSS  *  react-i18next (pt/en)    |
+--------------------------------+--------------------------------+
                                 | IPC (Tauri Core Invokes)
+--------------------------------v--------------------------------+
|                      Tauri / Rust Backend                       |
|  - HTTP Connection Pooling with Keep-Alive (reqwest)            |
|  - Atomic Configuration Persistence (fs::rename)                |
|  - MPD Process Supervisor & Dynamic mpd.conf Generation         |
|  - State & Queue File Caching (queue_cache.json)                |
+--------------------------------+--------------------------------+
                                 | UNIX Domain Socket
+--------------------------------v--------------------------------+
|                   Dedicated MPD Audio Daemon                    |
|       Configured for ALSA Exclusive or PipeWire Shared Output   |
+--------------------------------+--------------------------------+
                                 |
        +------------------------+------------------------+
        |                                                 |
        | Bit-Perfect PCM / DoP DSD                       | Shared PCM Audio
+-------v-------------------------+     +-----------------v---------------+
|   External Audiophile USB DAC   |     |    PipeWire / PulseAudio Server |
|   Direct Hardware (hw:CARD,DEV) |     |    System Mixed Output (default)|
+---------------------------------+     +---------------------------------+
```

---

## Installation & Distribution

Pre-built binaries are available in the [GitHub Releases](https://github.com/tiagoalam/sonante/releases) page.

### 1. Arch Linux / Manjaro
Install via the pre-compiled package or build using `makepkg`:

```bash
# Using your preferred AUR helper
yay -S sonante-bin
# or paru
paru -S sonante-bin
```

### 2. Debian / Ubuntu / Linux Mint (`.deb`)
Download the latest `.deb` package from Releases and install via `dpkg`:

```bash
sudo dpkg -i sonante_*_amd64.deb
sudo apt-get install -f # Resolve dependencies if needed
```

### 3. Universal Linux (`.AppImage`)
Download the `.AppImage`, make it executable, and run:

```bash
chmod +x sonante_*_amd64.AppImage
./sonante_*_amd64.AppImage
```

---

## Keyboard Shortcuts

| Shortcut | Description |
| :--- | :--- |
| **Space** | Play / Pause playback |
| **Right Arrow** | Next track |
| **Left Arrow** | Previous track |
| **Up Arrow** | Increase volume (+5%) |
| **Down Arrow** | Decrease volume (-5%) |
| **M** | Mute / Unmute |
| **Ctrl + F** | Focus global search bar |
| **Esc** | Close active modals / Queue drawer / Clear search |

---

## Building from Source

### Prerequisites

Ensure you have the following system libraries installed on your machine:

**Debian / Ubuntu:**
```bash
sudo apt update
sudo apt install -y build-essential curl wget file libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev mpd
```

**Arch Linux / Manjaro:**
```bash
sudo pacman -S --needed base-devel curl wget openssl gtk3 libayatana-appindicator librsvg alsa-lib mpd
```

### Build Instructions

1. Clone the repository:
```bash
git clone [https://github.com/tiagoalam/sonante.git](https://github.com/tiagoalam/sonante.git)
cd sonante
```

2. Install frontend dependencies:
```bash
npm install
```

3. Run in development mode:
```bash
npm run tauri dev
```

4. Build production packages:
```bash
npm run tauri build
```
Binaries and bundles will be placed in `src-tauri/target/release/bundle/`.

---

## Application Paths

Sonante organizes all user data, socket endpoints, and configurations within the user directory:

* `~/.config/sonante/config.json` — Hardware preferences, buffers, and Plex session tokens.
* `~/.config/sonante/favorites.json` — Unified favorites registry.
* `~/.config/sonante/queue_cache.json` — Persistent queue, playback state, and rich metadata cache.
* `~/.config/sonante/mpd.conf` — Dynamically generated MPD configuration.
* `~/.config/sonante/mpd.socket` — Dedicated MPD UNIX IPC control socket.
* `~/.config/sonante/library/` — Symlinked virtual directory mirroring all local library roots.

---

## License

Copyright (c) 2026 - present Tiago Alam. All rights reserved.

**Sonante is Free-to-Use Software (Source-Available):**
* You are free to download, install, build, and use this software on your personal machines at no cost.
* You may inspect and audit the source code for personal and security verification.
* **Restrictions:** You may **not** redistribute, sell, sub-license, host as a paid service, or republish full or substantial copies/derivatives of this project, its branding, or its compiled binaries without prior written authorization from the author.
