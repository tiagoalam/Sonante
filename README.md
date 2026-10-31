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
| **Online radio** | Worldwide station directory with name, country and genre filters; favorites, manual stream addresses and live playback. |
| **Playback** | Queue management, play/pause, next/previous, seek, session restoration without automatic playback, and desktop media-key integration through MPRIS2. |
| **Collection tools** | Local and Plex favorites, named playlists, availability-aware playlist playback, local album diagnostics, and reviewed FLAC tag editing with per-file results. |
| **Artwork & browsing** | Local and Plex album artwork, virtualized grids for large catalogs, and artwork caching with bounded work. |
| **Audio controls** | Shared and Direct output choices, device availability checks, ReplayGain settings, an explanatory Signal Path, and a shared Now Playing layout with visualization, an artwork queue, embedded, detached, and fullscreen modes. |

Sonante is designed for substantial, imperfect real-world libraries: multiple storage locations, multi-disc albums, CUE-based albums, varying metadata, and sources that may temporarily become unavailable.

## Installation — 0.5.9

Download the complete package and `SHA256SUMS.txt` from [Sonante 0.5.9](https://github.com/tiagoalam/Sonante/releases/tag/v0.5.9) for your distribution:

- Ubuntu 22.04: `sonante_0.5.9-1_amd64.deb` (`apt install ./sonante_0.5.9-1_amd64.deb`).
- Fedora 44: `sonante-0.5.9-1.x86_64.rpm` (`dnf install ./sonante-0.5.9-1.x86_64.rpm`).
- Arch Linux / Manjaro: `sonante-0.5.9-1-x86_64.pkg.tar.zst` (`pacman -U ./sonante-0.5.9-1-x86_64.pkg.tar.zst`).

Close Sonante before upgrading, and install with your distribution's package manager. Verify the supplied checksum first. Packages manage their own pinned audio runtimes and do not enable a system MPD service. Corresponding runtime source sidecars accompany the packages; they are not installed into `/usr`.

Ubuntu/Arch and Fedora intentionally use different runtime descriptors. Ordinary Tauri bundles and historical qualified-0.5.4 installers are not substitutes for the complete 0.5.9 release payload.

### System requirements

- A Linux desktop with **GTK 3 and WebKitGTK 4.1 API, library version 2.40 or newer** support.
- The package includes the pinned private **MPD** engines for playback, decoding, and indexing; format support depends on those builds.
- **ALSA** and `aplay` (usually from `alsa-utils`) for audio devices.
- **OpenSSL 3**, `xdg-open`, and an **XDG Desktop Portal** backend for integration with the desktop and folder selection.
- **WirePlumber / `wpctl` is optional**, used when available for Shared-mode volume control through PipeWire.
- **Shared EQ requires PipeWire** and an available stereo output. Native PulseAudio is not supported for EQ; activation is blocked when it is detected. Normal Shared playback remains available.
- The native Arch/Manjaro package requires the glibc version declared in its package metadata; it does not share the Ubuntu GLIBC 2.35 baseline.
- Artwork decoding depends on the supported image loaders. Invalid images use the existing placeholder; decoder-specific validation is recorded separately.

Exact native dependencies vary by distribution and are checked as part of packaging. The Ubuntu 22.04 build is additionally checked against a maximum required **GLIBC 2.35** symbol version; this does not guarantee compatibility with every distribution or device.

The current DEB/RPM packaging requires WebKitGTK **2.40 or newer**: Tauri/Wry uses [`webkit_uri_scheme_request_get_http_body`, introduced in 2.40](https://webkitgtk.org/reference/webkit2gtk/2.42.1/method.URISchemeRequest.get_http_body.html). The **4.1** API/SONAME does not imply that library version. A Fedora 37 installation with WebKitGTK 2.38.2 accepted the older v0.4.9 RPM but failed at startup with this missing symbol. Versioned package requirements prevent that unsupported dependency combination; they do not establish full Fedora compatibility or replace installation/startup validation with suitable dependencies.

The prepared Fedora recipe keeps the rich dependency `(webkit2gtk4.1 >= 2.40)` and the full Fedora compatibility dependency set. Final packages are assembled from the verified payload; ordinary Tauri bundler output is not a release artifact.

Fedora uses the separate compat3 runtime descriptor: private bzip2/sndio ABIs, system Nettle >=3.10, and no private obsolete Nettle library. Dependency names and actual ELF requirements are checked again for the final RPM.

## Getting started

1. **Choose an audio output.** Shared mode uses the system's default ALSA route; Direct mode targets a selected ALSA hardware endpoint.
2. **Add your music.** Register one or more local folders, connect a Plex server, or use both.
3. **Browse your collection.** Explore albums, artists, years, genres, and the source-specific views; add tracks to the queue or save them to a playlist.
4. **Adjust playback.** Use the player controls, your desktop media keys, or the Now Playing window.

The first-run wizard guides you through these choices. Plex sign-in uses an external authorization flow rather than asking you to paste a token into the interface.

## Library management

### Local collections

Sonante presents multiple configured roots through a managed virtual library indexed by MPD. Library changes use guarded preparation and verification rather than rewriting the original music files simply to make them discoverable. Mounted volumes and network shares may be used as local sources if they are accessible to the process.

Startup reuses the saved local index. Use **Check for changes** to request a library update; adding a folder indexes the new source, and removing a folder does not reindex surviving sources.

Local browsing includes **Albums, Artists, Years, Genres, and Folders**. Album handling also accounts for multi-disc layouts and supported physical/virtual CUE representations. The interface is designed to remain usable with large catalogs through pagination and virtualized rendering.

**Tag editing:** inspect and edit supported physical FLAC tracks individually or review album-level changes before applying them. Writes are performed independently per file, with results shown for each file; a later failure does not undo earlier writes. Plex tracks, virtual CUE tracks, and unsupported formats are not writable through this editor.

**Library Doctor:** inspect a local album for metadata inconsistencies and review the evidence. Diagnosis does not change files or automatically select corrections.

New folder selections reject overlapping parent/child roots, including canonical aliases. Existing overlapping configurations remain readable; Sonante does not automatically merge or migrate their references.

### Plex libraries

Plex connectivity is associated with a server's stable identity, not just a temporary URL. Sonante can use validated server connections and resolve playback references when needed. It offers **Albums, Artists, Years, Genres, and Collections**, along with favorites and playlists spanning supported local and Plex items.

Artwork requests are handled through the backend; authenticated Plex URLs and access tokens are not exposed as frontend image URLs. A missing or unreachable Plex source is reported as unavailable rather than being treated as a permanently empty collection.

## Explore, search, and Now Playing

**Explore** is a single navigation entry. With both sources configured, prominent Local/Plex tabs select local suggestions or Plex discovery. Local suggestions can target one local source or all local libraries independently of the Local browser selection. Startup restores the main Local or Plex section rather than reopening a suggestion view. Favorites use the same source tabs.

**Plex discovery** offers library-aware shelves and finite mixes of up to 100 tracks: Library Mix, Decade Mix (using the album release year), and Style Mix. Open a mix to preview it, play it, generate another selection, or save that exact selection as a named playlist. Sources and availability remain associated with their original libraries.

**Unified search** (Ctrl+K) searches Local and Plex albums, artists, and tracks together, with source/type filters and keyboard navigation. Results open their original source context.

**Now Playing** can fill the main content area or open as a detached window with fullscreen support. Choose Audiophile, Psychedelic, Sleeve or Poster from the layout selector or keys 1–4. Audiophile includes revised VU meters, a circular dial option and spectrum views; Psychedelic offers Aurora, Mandala and Cosmos scenes with an economy mode. Double-click the artwork to toggle fullscreen. The detached fullscreen player can hide controls while playing, and each layout remembers whether its queue panel is open. The queue opens on the right with album artwork and can be hidden. Repeat queue, repeat track, and shuffle controls share the MPD playback state across player presentations. The queue supports filtering, jumping to the current occurrence, and duration estimates when the playback mode permits them. Track context menus and ellipsis buttons can insert a track after the current queue position, append it, or append its identified album without replacing the queue or starting playback on an empty queue. Existing shuffle/repeat settings are retained. Reorder queue occurrences by dragging or with the keyboard, move a queued track next, and remove other occurrences while protecting the currently playing or paused item.

Use Ctrl/Shift or the selection checkboxes to add multiple tracks to the queue or a playlist. Album menus load tracks only when an action needs them. **Recent history** keeps up to 100 Local/Plex tracks locally and can be cleared or disabled.

The sleep timer offers 15, 30 or 60 minutes from the player bar and Now Playing. It shows the remaining time and can be canceled; expiry stops playback without clearing the queue. Timers are session-only. The main Plex library also shows loading progress and a retry action when loading fails.

## Online radio

**Radio** searches the community [Radio Browser](https://www.radio-browser.info/) directory by station name, country and genre. Results are paginated; **Clear filters** immediately restores the unfiltered list. Save favorites or add a direct HTTP/HTTPS audio stream address. Manually added stations can be edited or deleted in Favorites. A website address is not an audio stream.

Playback uses the selected Sonante output and shows **Live**, with station/track metadata when supplied by the broadcaster. Live radio cannot be seeked; Stop ends the stream. Availability, codec support and compatibility with strict Direct output vary by station; try another station or Shared when necessary. Radio streams do not enter Local/Plex listening history, playlists or automatic session restoration. Directory searches and explicit station playback contact the directory and/or broadcaster; opening unrelated library sections does not query the radio directory.

## Appearance and remote control

Choose from six themes: Dark, Light, Cyber, Summer, Zen, and Nocturne. Compact previews are available in Preferences and the first-run wizard. Catalog counts reuse existing metadata.

The optional [UPnP receiver](docs/UPNP_RECEIVER.md) lets compatible controllers such as BubbleUPnP send music to Sonante. It is disabled by default and uses the selected audio output. This is a renderer, not a media server or a TIDAL/Qobuz Connect implementation.

## Audio: what Shared and Direct mean

Sonante uses its **own supervised MPD process** as the actual decoder and playback engine. The application itself does not implement a separate PCM decoder/output pipeline.

| | Shared | Direct |
| --- | --- | --- |
| **MPD output** | ALSA `default` | Selected ALSA hardware endpoint (typically `hw:CARD=...,DEV=...`) |
| **System audio** | May coexist with other applications through PipeWire, PulseAudio, or dmix, depending on the host | Requests access to a hardware endpoint; availability and exclusivity depend on the device and host |
| **Volume** | PipeWire sink via `wpctl` when available and verified; otherwise MPD software volume | Volume is controlled on the equipment; Sonante provides explicit software mute |
| **Resampling / mixing** | May occur in the host audio stack | Depends on the effective MPD/ALSA/device configuration |

The interface offers **ReplayGain** controls and can request **DoP** when configured for a compatible Direct output. DoP is not the same as native DSD, and enabling it does not prove that a DAC received DSD data.

**Sonante does not claim verified bit-perfect output.** The displayed sample rate, bit depth, and channel information are **reported by MPD**, not measured at the DAC. Software volume, ReplayGain, resampling, and external mixing can change the actual signal. The Signal Path view is an explanation of the selected route and known status, **not an end-to-end audio measurement**.

For decoder support of the packaged Direct engine, inspect `/usr/lib/sonante/runtimes/v1/mpd-direct --version`. A system `mpd` may be a different build.

## Privacy and application data

Sonante stores its per-user configuration and library state locally. Typical paths (under the XDG configuration directory) include:

| File / directory | Purpose |
| --- | --- |
| `~/.config/sonante/config.json` | Audio, source, and Plex settings |
| `~/.config/sonante/favorites.json` | Favorite albums |
| `~/.config/sonante/queue_cache.json` | Cached queue metadata |
| `~/.config/sonante/radio-favorites.json` | Favorite and manually added radio stations |
| `~/.config/sonante/listening-history.json` | Bounded Local/Plex listening history |
| `~/.config/sonante/mpd.conf` and `mpd-v2.db` | Private MPD configuration and catalog |
| `~/.config/sonante/library/` | Managed virtual library |
| `$XDG_RUNTIME_DIR/sonante/` | Private MPD runtime socket and process metadata, when the runtime directory is available |

Configuration directories and sensitive files are created with restrictive Linux permissions. Plex credentials require special handling: **do not share configuration files or diagnostic output containing tokens**. Uninstalling the application package does not itself remove your personal library or configuration files.

## Build from source

Development requires **Node.js 22**, npm, a stable Rust toolchain, and the native development dependencies for Tauri 2, GTK 3, WebKitGTK 4.1, ALSA, and OpenSSL. See the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/) and the [CI workflow](https://github.com/tiagoalam/Sonante/blob/main/.github/workflows/ci.yml) for the Ubuntu build environment.

For a development compilation of the verified 0.5.9 source archive without installation, extract the supplied source archive and run the normal build commands. This compilation alone does not create the complete release packages.

```bash
# Extract the supplied, SHA-verified source archive first.
cd sonante
# The supplied sources already identify version 0.5.9; no unpublished tag is required.
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
python3 -m unittest discover -s tests -p '*packaging_test.py'
```

Development CI now produces **`sonante-complete-linux-packages`** after both native build jobs pass. This artifact contains complete DEB, Fedora RPM and Arch installers, corresponding sources and `SHA256SUMS.txt`. The separate `sonante-compilation-*-inputs` artifacts are intermediate build inputs. See [complete-package assembly](packaging/COMPLETE_PACKAGES.md) for provenance, checks and local reproduction. The manual workflow uses the same assembly; it does not publish a release. The 0.5.9 release uses revision-1 packages; CI rebuilds do not replace published assets. AppImage is outside this scope.

## Project status and scope

Sonante prioritizes **reliable playback, conservative MPD lifecycle management, safe library changes, and clear audio reporting** over claiming support for every possible codec or audio feature. Platform and hardware behavior must be tested rather than inferred from a successful build.

Local tag editing is limited to supported physical FLAC files and reports independent per-file results. Availability of codecs, network sources, and DAC capabilities depends on the configured system. The 0.5.9 release targets DEB, RPM, and Arch packages for x86_64; each requires its own final installation and startup evidence.

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
