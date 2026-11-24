# Sonante 0.7.2

Sonante 0.7.2 is a stable Linux release for Ubuntu/Debian, Fedora and Arch Linux/Manjaro.

## What's new

- **WavPack:** Local SQLite catalog support for WavPack audio and supported WavPack/CUE albums, including virtual tracks for single-image and multi-file CUE layouts.
- **Subsonic/OpenSubsonic:** a generic Subsonic source that keeps existing Navidrome connections working. Manual scenarios were checked with Navidrome, Gonic 0.22.0 and Ampache 8.4. Compatibility depends on the endpoints and capabilities each server provides; this is not a certification of every implementation.
- **Personal Collections:** save albums from Local, Plex and Subsonic together, with references retained when a source or album is temporarily unavailable.
- **Discotheque Carousel:** browse album covers with keyboard, pointer and touch navigation. Opening an album does not start playback.
- **About / Help:** the existing About dialog now includes a dedicated keyboard-shortcuts tab.
- **Library and interface refinements:** improved source presentation, metadata handling, artwork loading and compatibility behavior.

## Linux packages

Download one package for your distribution and verify the package checksum before installation:

- Ubuntu 22.04 or newer and compatible derivatives: DEB.
- Fedora 44 or newer and compatible derivatives: RPM.
- Current Arch Linux / Manjaro: Arch package requiring the declared GLIBC 2.43 baseline and OpenSSL 3 SONAMEs.

The Ubuntu 22.04 DEB, Fedora 44 RPM and current Arch package passed the project's package and installation/loader checks. The release was also installed and exercised manually on Manjaro. These results cover the tested environments only; they do not guarantee compatibility with every derivative, driver or audio device.

Run sha256sum -c SHA256SUMS.txt with the three downloaded packages beside the checksum file. The checksum file covers only those three package assets.

## Source and audio notes

The public source tag contains the Sonante application and Linux package build inputs. The release's [source and provenance guide](docs/releases/0.7.2/README.md) links the platform build receipts and the corresponding public source archives for the bundled MPD and support-library runtimes. The same runtime profiles were retained for these packages.

Shared and Direct output remain available. Sonante does not certify general bit-perfect output. The package CI performed ABI, installation and loader checks; those checks are not a measurement of audio at a DAC.

Windows remains in development and is not included in this Linux release.
