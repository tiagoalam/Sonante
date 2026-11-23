# Sonante 0.7.1

Sonante 0.7.1 is a stable Linux release with a persistent local music catalog and complete installers for Ubuntu/Debian, Fedora and Arch.

## What's new

- Local folders and mounted shares can be registered in the persistent SQLite catalog. Importing folders saved by an earlier public version is a manual action; it does not move, rename or edit music files.
- Existing favorites and settings are retained. A Local favorite without an exact safe match may need to be added again. The old queue is not automatically rebound to SQLite locators.
- Lyrics can be read from a matching local `.lrc` sidecar or searched from the online lyrics service. You can choose a result for a track; remote lyrics are not bundled with the application.
- ParametricEQ text files can be imported into a new local EQ preset. The optional OPRA browser displays catalog profiles with source and attribution details; those profiles remain read-only and are not converted or applied.
- DSD64 mono and stereo samples can be compared after conversion to PCM. This is a comparison path, not native DSD playback.
- Library navigation, metadata tools, artwork feedback and Now Playing controls have been refined.

## Linux packages

The DEB, RPM and Arch packages passed their package integrity checks and installation/loader checks on Ubuntu 22.04, Fedora 44 and current Arch with its OpenSSL 3 compatibility runtime. These checks do not certify every derivative distribution or audio device.

The release includes SHA-256 checksums, application source archives and provenance receipts. Runtime source archives used by the bundled MPD engines are identified by filename, size and SHA-256 in the platform source-material manifests and remain available from the earlier public source release.

Physical audio and hardware validation was not performed for this release. Sonante makes no bit-perfect output guarantee.

## Previous public release

The previous public release was Sonante 0.6.3. See the [0.6.3 release](https://github.com/tiagoalam/Sonante/releases/tag/v0.6.3) for its original notes.
