<div align="center">

# Sonante

**Audiophile Bit-Perfect Desktop Audio Player for Linux**

*Direct hardware ALSA routing, native DSD/PCM stream, and seamless Plex Media Server integration.*

[![Release](https://img.shields.io/github/v/release/tiagoalam/sonante?style=flat-square&color=E5A00D)](https://github.com/tiagoalam/sonante/releases)
[![Platform](https://img.shields.io/badge/Platform-Linux%20(ALSA)-blue?style=flat-square)](#requirements)
[![Backend](https://img.shields.io/badge/Backend-Rust%20%7C%20Tauri-orange?style=flat-square)](#architecture)
[![Frontend](https://img.shields.io/badge/Frontend-React%2018%20%7C%20TypeScript-blueviolet?style=flat-square)](#architecture)
[![Engine](https://img.shields.io/badge/Audio%20Engine-Dedicated%20MPD-white?style=flat-square)](#audiophile-audio-engine)
[![License](https://img.shields.io/badge/License-Freeware%20%2F%20Source--Available-yellow?style=flat-square)](#license)

</div>

---

## Overview

**Sonante** is a desktop music player crafted specifically for audiophiles and high-fidelity audio enthusiasts on Linux. By bypassing OS-level resampling, software mixers (PulseAudio/PipeWire), and audio degradations, Sonante communicates directly with your dedicated USB DAC through unadulterated ALSA hardware nodes (`hw:X,Y`).

Sonante seamlessly unifies your offline high-resolution local collection (across multiple drives and directories) and remote **Plex Media Server** audio libraries into an elegant, dark, responsive interface.

---

## Key Features

### Audiophile Audio Engine
* **Direct ALSA Exclusive Access:** Feeds raw PCM and DSD data straight to the DAC without kernel mixer intervention.
* **Native DSD & Hi-Res PCM:** Full bit-perfect playback up to **384 kHz / 32-bit PCM** and **DSD over PCM (DoP)** up to DSD128.
* **Supervised Background MPD Core:** A dedicated, lightweight Music Player Daemon (MPD) instance is managed internally via a secure UNIX socket, fully decoupling audio decoding from UI rendering.
* **Configurable RAM Audio Buffer:** Configurable ring buffer (4 MB to 32 MB) to eliminate disk I/O jitter and network streaming dropouts.
* **Pure ReplayGain Control:** Bit-perfect output when disabled, with optional Track or Album mode normalization.

### Plex Media Server Integration
* **Official OAuth / PIN Authentication:** Secure login via web browser with automatic polling and local token persistence.
* **LAN Auto-Discovery & Direct Play:** Automatically detects whether the server is local or remote, prioritizing local network IP routes for maximum throughput.
* **Lossless Direct Streaming:** Direct stream playback of FLAC, ALAC, and DSD tracks without server-side transcoding.
* **Unified Remote Navigation:** Browse Plex Music Libraries, Collections, Artist Discographies, and perform fast instant search with debounced indexing.

### Local Music Management
* **Multi-Directory Aggregation:** Merge arbitrary local folders, internal disks, and external USB drives under a unified symlink structure.
* **Album Deduplication:** Consolidates loose audio files into organized album collections.
* **LRU Caching & Lazy Loading:** Visual artwork is lazily loaded using an `IntersectionObserver` coupled with an in-memory Least-Recently-Used (LRU) cover cache to minimize RAM consumption.

### UI & User Experience
* **Fully Internationalized (i18n):** Native support for **English (en-US)** and **Portuguese (pt-BR)** with real-time switching across the entire UI.
* **Interactive First-Run Wizard:** Guides the user through DAC hardware selection, local library configuration, and Plex connection.
* **Unified Favorites:** Persistent favorites system across both local albums and Plex libraries with active offline availability tracking.
* **Global Keyboard Shortcuts:** Fast control for common playback, volume, and search actions.

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                    Frontend (React 18 + Vite)                   │
│     Lucide Icons  •  Tailwind CSS  •  react-i18next (pt/en)     │
└────────────────────────────────┬────────────────────────────────┘
                                 │ IPC (Tauri Core Invokes)
┌────────────────────────────────▼────────────────────────────────┐
│                       Tauri / Rust Backend                      │
│  - HTTP Connection Pooling with Keep-Alive (reqwest)            │
│  - Atomic Configuration Persistence (fs::rename)                │
│  - MPD Process Supervisor & Dynamic mpd.conf Generation        │
└────────────────────────────────┬────────────────────────────────┘
                                 │ UNIX Domain Socket
┌────────────────────────────────▼────────────────────────────────┐
│                    Dedicated MPD Audio Daemon                   │
│               Direct Hardware Route to ALSA (hw:X,Y)            │
└────────────────────────────────┬────────────────────────────────┘
                                 │ Bit-Perfect PCM / DoP DSD
┌────────────────────────────────▼────────────────────────────────┐
│                   External Audiophile USB DAC                   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Installation & Distribution

Pre-built binaries are available in the [GitHub Releases](https://github.com/tiagoalam/sonante/releases) page.

### 1. Arch Linux / Manjaro
You can install via the pre-compiled package or build using `makepkg`:

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
* `~/.config/sonante/mpd.conf` — Dynamically generated MPD configuration.
* `~/.config/sonante/mpd.socket` — Dedicated MPD UNIX IPC control socket.
* `~/.config/sonante/library/` — Symlinked virtual directory mirroring all local library roots.

---

## License

Copyright (c) 2024-present Tiago Alam. All rights reserved.

**Sonante is Free-to-Use Software (Source-Available):**
* You are free to download, install, build, and use this software on your personal machines at no cost.
* You may inspect and audit the source code for personal and security verification.
* **Restrictions:** You may **not** redistribute, sell, sub-license, host as a paid service, or republish full or substantial copies/derivatives of this project, its branding, or its compiled binaries without prior written authorization from the author.
