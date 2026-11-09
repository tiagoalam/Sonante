<div align="center">

<img src="app-icon.svg" alt="Sonante icon" width="88" />

# Sonante

### Your music. Your way.

**A modern, open-source music player that brings your music collection, Plex and Navidrome together.**

[Website](https://tiagoalam.github.io/Sonante/) · [Download for Linux](https://github.com/tiagoalam/Sonante/releases/latest) · [Features & documentation](docs/LINUX_GUIDE.md) · [Report an issue](https://github.com/tiagoalam/Sonante/issues/new/choose)

</div>

Sonante is designed for people who enjoy exploring, organizing and listening to their own music. Browse local folders and network-mounted collections alongside your Plex and Navidrome libraries, build playlists, and choose how you want to listen—all in one desktop application.

**Available now:** Linux x86_64. **In development:** Windows. Other platforms are not yet released.

## Why Sonante?

| | |
| --- | --- |
| **One home for your collection** | Browse local music, Plex libraries and Navidrome in a unified interface, while keeping each source identifiable. |
| **Rediscover familiar music** | Explore albums, artists, years and genres, with collection-based suggestions, mixes and search. |
| **Make listening your own** | Manual and smart playlists, favorites, queue controls, history and desktop media keys. |
| **Choose your experience** | Simple and Full modes, six themes, multiple catalog layouts, and immersive Now Playing views. |
| **Take care of your library** | Local metadata diagnostics, reviewed FLAC tag editing and tools for managing large collections. |
| **Control the playback route** | MPD-based playback, Shared and Direct output options, device selection, ReplayGain and an informative Signal Path. |

Sonante also offers online radio, optional Last.fm and ListenBrainz scrobbling, an opt-in UPnP receiver, and experimental listening and remote-control tools. Feature availability varies by mode and source. See the [full Linux feature guide](docs/LINUX_GUIDE.md).

## See Sonante

Visit the [official website](https://tiagoalam.github.io/Sonante/) for screenshots and an overview of the interface.

## Download and install

Get the latest published Linux release and its `SHA256SUMS.txt` file from [GitHub Releases](https://github.com/tiagoalam/Sonante/releases/latest).

The current release is **v0.6.3**, with packages for:

| Distribution | Package format |
| --- | --- |
| Ubuntu 22.04 | `.deb` |
| Fedora 44 | `.rpm` |
| Arch Linux / Manjaro | `.pkg.tar.zst` |

Download the package matching your distribution, verify its checksum, close Sonante if it is running, and install it using your distribution's package manager. The release packages include Sonante's supervised private MPD runtime; you do not need to enable a system MPD service.

**Compatibility matters:** Linux packages require a supported GTK 3 / WebKitGTK 4.1 environment (WebKitGTK 2.40 or newer), ALSA and distribution-specific native dependencies. Other Linux distributions may work but are not automatically validated. Read [installation requirements and troubleshooting details](docs/LINUX_GUIDE.md#installation--063) before installing on an untested system.

## Getting started

1. Install the appropriate package from [Releases](https://github.com/tiagoalam/Sonante/releases/latest).
2. Open Sonante and follow the welcome wizard.
3. Choose a Shared or Direct audio output.
4. Enable **Local**, **Plex** and/or **Navidrome** in **Preferences → Sources**.
5. Start browsing your albums, build a queue and enjoy your collection.

For source setup, library management, audio behavior and privacy details, see the [Linux user and technical guide](docs/LINUX_GUIDE.md).

## Simple or Full? It's your choice.

**Simple** keeps the core music experience: your sources, search, favorites, playlists, queue, themes and Now Playing.

**Full** adds more advanced features such as Explore, smart playlists, EQ and UPnP. Experimental tools remain opt-in. Switching modes preserves your settings, collection and playback state.

## Status and feedback

Sonante is open source and under active development. The Linux version is available for real-world use and feedback; the Windows port is still being developed and **is not part of the current Linux release**.

If something does not work, please [open a bug report](https://github.com/tiagoalam/Sonante/issues/new/choose). Include the Sonante version, distribution, audio device, reproduction steps and relevant diagnostics. **Remove personal information, library paths and server credentials before sharing logs.**

For changes in the latest release, see [Release Notes](RELEASE-NOTES.md) and [Changelog](CHANGELOG.md).

## For developers

Sonante uses **React, TypeScript, Tauri and Rust**, with a supervised **Music Player Daemon (MPD)** for ordinary playback. The current Linux audio implementation uses **ALSA**.

- [Linux technical guide and build instructions](docs/LINUX_GUIDE.md)
- [UPnP receiver](docs/UPNP_RECEIVER.md)
- [Remote control API](docs/remote-control-api.md)
- [Gapless validation notes](docs/GAPLESS_VALIDATION.md)
- [Third-party notices](THIRD_PARTY_NOTICES)

## License

Sonante is distributed under the [GNU General Public License v3.0](LICENSE).
