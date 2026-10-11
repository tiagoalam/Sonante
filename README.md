# Sonante

Sonante is a Linux desktop music player for local libraries and Plex Media Server. Its frontend is built with React and TypeScript; Tauri and Rust provide the native backend, while a dedicated MPD process handles playback through ALSA.

The project targets Linux. Packaging candidates for 0.4.9 cover DEB, RPM and Arch Linux on x86_64; distribution validation is described below.

## Features

- Local libraries assembled from multiple directories and indexed by MPD.
- Plex authentication, music libraries, albums, artists, search, collections, and remote playback.
- Persistent Plex server identity with dynamic selection of reachable local, remote-direct, or relay connections.
- Queue controls, play/pause, seek, next/previous navigation, and mirrored queue metadata.
- Unified local and Plex album favorites.
- Collection artwork mosaics and secure Plex artwork loading with lazy viewport loading, bounded concurrency, and an in-memory LRU cache.
- Shared and Direct ALSA output modes.
- Volume control through PipeWire, ALSA hardware mixers, or MPD software mixing according to the active backend.
- ReplayGain modes provided to MPD: off, track, and album.
- English and Brazilian Portuguese interface.

## Audio model

MPD is the playback engine. Sonante generates a private MPD configuration, starts and supervises the process, communicates through a per-user Unix socket, and sends decoded audio to an ALSA output selected by the user.

### Shared

Shared mode points MPD at the ALSA `default` device so the host audio stack can provide coexistence with other applications. Mixing and resampling behavior are controlled by the host configuration, which may use PipeWire, PulseAudio, or ALSA dmix.

When PipeWire is configured and `wpctl` successfully probes the default sink, Sonante disables MPD mixing and reads/writes volume through that PipeWire sink. If the initial probe is unavailable, Sonante explicitly uses MPD's software mixer. A later `wpctl` failure makes volume temporarily unavailable instead of silently changing mixers.

### Direct

Direct mode points MPD at the selected ALSA hardware endpoint, normally an address such as `hw:CARD=...,DEV=...`.

Sonante uses an ALSA hardware mixer only when it can associate the PCM endpoint with a mixer device and find exactly one usable playback-volume control. Missing, invalid, or ambiguous controls fall back to MPD software volume.

Direct mode can request DoP in the generated MPD configuration. That setting is a request to MPD and does not confirm native DSD operation, the format received by the DAC, or an unmodified end-to-end signal path. ReplayGain and software volume also change the signal path.

### Formats

Audio format support comes from the installed MPD build and its enabled decoder plugins. Sonante does not ship codecs or maintain a separate extension whitelist. Use `mpd --version` on the target system to inspect the formats and decoder plugins available there. The format displayed by Sonante is the value reported by MPD, not a measurement at the ALSA endpoint or DAC.

## Plex and security

- A Plex server is persisted by its `machineIdentifier`, independently of a transient connection URL.
- The backend validates and selects an available connection and resolves stable media references only when playback begins.
- Artwork is represented publicly as a stable server ID plus a relative Plex path. Authentication tokens remain in the Rust backend and are not placed in image URLs or frontend state.
- Legacy authenticated artwork in favorites and the queue cache is migrated when the server identity is known; otherwise only the unsafe artwork is discarded.
- Large artwork uses a bounded 600×600 Plex thumbnail fallback while preserving MIME and response-size checks.
- On Linux, Sonante-created configuration directories use mode `0700`; `config.json`, `favorites.json`, `queue_cache.json`, and atomic-save temporary files use mode `0600`.

## Runtime dependencies

Sonante currently depends on system components rather than bundling them:

- MPD is required for playback and library indexing.
- ALSA and `aplay` (`alsa-lib` and `alsa-utils` on Arch) are used for output and device discovery.
- GTK 3 and WebKitGTK 4.1 support the Tauri desktop application.
- `xdg-open` is used to open the Plex authentication page.
- OpenSSL 3 is used by the native HTTP stack. RPM candidates carry the actual SONAME and symbol requirements of the compiled executable.
- XDG Desktop Portal and a suitable desktop backend (GTK, GNOME or KDE) provide the folder chooser. The generic GTK backend is recommended when no desktop-specific backend is available.
- WirePlumber is optional. When its `wpctl` utility is available and PipeWire probing succeeds, Sonante uses it for Shared volume control; otherwise the documented MPD software fallback remains available.

## Installation

### Arch Linux / Manjaro

After the 0.4.9 packages are validated and published, verify the downloaded package against its `SHA256SUMS-arch.txt` and install it with pacman:

```bash
sudo pacman -U ./sonante-0.4.9-1-x86_64.pkg.tar.zst
```

The package does not configure a system-wide MPD service. Sonante starts and supervises its own MPD process.

### Build from source

Install the build and runtime dependencies on Arch Linux or Manjaro:

```bash
sudo pacman -S --needed base-devel git nodejs npm rust \
  gtk3 webkit2gtk-4.1 \
  alsa-lib alsa-utils openssl mpd xdg-utils xdg-desktop-portal xdg-desktop-portal-gtk
```

WirePlumber is optional:

```bash
sudo pacman -S --needed wireplumber
```

Clone and build the application without generating distribution bundles:

```bash
git clone https://github.com/tiagoalam/sonante.git
cd sonante
npm ci
npm run tauri -- build --no-bundle -- --locked
```

The executable is written to `src-tauri/target/release/sonante`.

To build the Arch package from a published 0.4.9 tag (the recipe fetches that exact tag):

```bash
cd sonante-arch
makepkg -s
```

## Development

Install JavaScript dependencies:

```bash
npm ci
```

Run the Tauri application in development mode:

```bash
npm run tauri dev
```

Run the available checks and tests:

```bash
./node_modules/.bin/tsc --noEmit
node tests/plexArtworkCache.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

Build only the frontend:

```bash
npm run build
```

Build the native release executable without producing `.deb` or AppImage bundles:

```bash
npm run tauri -- build --no-bundle -- --locked
```

## Persistent and runtime files

- `~/.config/sonante/config.json`: application, audio, and Plex configuration.
- `~/.config/sonante/favorites.json`: local and Plex favorites.
- `~/.config/sonante/queue_cache.json`: mirrored queue metadata; it is not an MPD playback-state restore file.
- `~/.config/sonante/mpd.conf` and `mpd.db`: generated MPD configuration and database.
- `~/.config/sonante/library/`: virtual library of symlinks to configured local roots.
- `$XDG_RUNTIME_DIR/sonante/mpd.socket` and `mpd.pid`: per-user MPD runtime endpoints. A private directory below the Sonante configuration directory is used when `XDG_RUNTIME_DIR` is unavailable.

## Packaging status

The 0.4.9 packaging configuration is prepared; this checkout does not itself publish a release. CI builds DEB/RPM on Ubuntu 22.04 and builds the Arch package natively in an Arch container. Manual dispatch of the release workflow builds candidates only. Upload is gated on an already-published GitHub Release and successful tests/inspection for all three formats.

Ubuntu candidates are checked for ELF dependencies and required GLIBC symbol versions (maximum 2.35). This check does not guarantee compatibility of every native library on other distributions. Arch candidates use the Arch toolchain and have their own ABI report, without a 2.35 claim. RPM dependency requirements are generated from the executable, including versioned OpenSSL/GLIBC symbols.

Before publication, validate installation, upgrade, desktop entry/icons, folder chooser, playback and uninstall on clean Ubuntu/Debian, Fedora and Arch/Manjaro environments. Fedora must have MPD and the required native ABI available in its configured repositories; the package does not enable third-party repositories. WirePlumber remains optional. No package configures or enables a system MPD service, and personal Sonante files are not package payloads.

AppImage remains outside this release preparation because MPD/runtime bundling has not been validated.

### Debian / Ubuntu candidate installation

From the directory containing the downloaded DEB and `SHA256SUMS-ubuntu.txt`, verify checksums (the manifest also lists the RPM; download both for this command), then let apt resolve dependencies:

```bash
sha256sum -c SHA256SUMS-ubuntu.txt
sudo apt install ./Sonante_0.4.9_amd64.deb
```

### Fedora candidate installation

From the same directory containing the DEB, RPM and checksum manifest:

```bash
sha256sum -c SHA256SUMS-ubuntu.txt
sudo dnf install ./Sonante-0.4.9-1.x86_64.rpm
```

Installing a newer local package through apt/dnf/pacman upgrades the existing package. Package removal uses `sudo apt remove sonante`, `sudo dnf remove sonante` or `sudo pacman -R sonante`; it does not delete personal configuration/music. These are candidate procedures, not a claim of completed distribution testing.

### Reproduce packaging checks

```bash
node scripts/verify-packaging.mjs
python3 -m unittest discover -s tests -p 'packaging_test.py'
npm run test:frontend
cargo check --manifest-path src-tauri/Cargo.toml --locked
npm run tauri -- build --no-bundle -- --locked
bash scripts/inspect-elf.sh src-tauri/target/release/sonante 2.35
python3 scripts/rpm-elf-deps.py src-tauri/target/release/sonante /tmp/sonante-rpm-config.json
npm run tauri -- bundle --bundles deb,rpm --config /tmp/sonante-rpm-config.json
bash scripts/inspect-packages.sh 0.4.9
```

Use an Ubuntu 22.04 build environment for that ABI ceiling. The RPM overlay is created exclusively: choose a fresh filename if it already exists. Arch CI uses the committed checkout with checksummed local sources, preserving the recipe's build/install functions and avoiding dependence on a tag during pre-release CI. It refuses a working-tree/source version mismatch.

## License

Copyright © 2026 Tiago Alam. All rights reserved.

The repository currently has no standalone `LICENSE` file and is not published under an OSI-approved open-source license. The source may be inspected and built for personal use, but redistribution, sale, sublicensing, paid hosting, or republication of substantial copies or derivatives requires prior authorization from the author.
