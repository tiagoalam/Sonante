# Sonante 0.7.2 source, provenance and verification

This directory keeps the release records versioned in the public repository.
The release page uploads only the three install packages and SHA256SUMS.txt;
the receipts and supporting records below are available here instead.

## Download assets

The 0.7.2 release provides:

- sonante_0.7.2-1_amd64.deb
- sonante-0.7.2-1.x86_64.rpm
- sonante-0.7.2-1-x86_64.pkg.tar.zst
- SHA256SUMS.txt

SHA256SUMS.txt covers only the three package files. Download the matching
package and checksum file, then run sha256sum -c SHA256SUMS.txt in the same
directory.

## Corresponding source

The public v0.7.2 Git tag contains the Sonante application, build inputs,
LICENSE, third-party notices, locked dependency manifests and package
installation/build scripts. GitHub provides source archives for the tag:

- [v0.7.2 source archive (tar.gz)](https://github.com/tiagoalam/Sonante/archive/refs/tags/v0.7.2.tar.gz)
- [v0.7.2 source tree](https://github.com/tiagoalam/Sonante/tree/v0.7.2)

The application is distributed under GPL-3.0-only. The package build inputs
and Linux source snapshot are preserved in the public tag. The release does
not include a Windows binary.

The packages also contain pinned MPD and support-library runtimes. The
platform source-material manifests link their corresponding source materials,
which remain publicly downloadable from the earlier v0.5.5 source release:

- [Runtime source materials for Ubuntu and Arch](https://github.com/tiagoalam/Sonante/releases/download/v0.5.5/runtime-source-materials-consumer-clock.tar.zst)
- [Additional Fedora compatibility sources](https://github.com/tiagoalam/Sonante/releases/download/v0.5.5/fedora-compat-source-materials-20261020.3.tar.zst)
- [v0.5.5 source release and its notices](https://github.com/tiagoalam/Sonante/releases/tag/v0.5.5)

The platform manifests record the filenames, public URLs, byte counts and
SHA-256 values. The runtime source archive includes the build inputs and
license notices for the MPD engines and bundled support components. Fedora
also uses the additional compatibility source archive identified above.
These source archives are not separate assets on the 0.7.2 release page
because the same pinned runtimes were carried forward unchanged. The pinned
MPD Direct SHA-256 is
7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1; the MPD EQ
SHA-256 is 6b9cb9820c1704175af312377fae6f20ad80486f69ee17cd190bc6390bc1d5ff.
The platform runtime-manifest hashes are recorded in the provenance JSON.

## Build and validation records

- [Ubuntu build receipt](BUILD-RECEIPT-ubuntu.json)
- [Fedora build receipt](BUILD-RECEIPT-fedora.json)
- [Arch build receipt](BUILD-RECEIPT-arch.json)
- [Ubuntu source-material manifest](SOURCE-MATERIALS-ubuntu.json)
- [Fedora source-material manifest](SOURCE-MATERIALS-fedora.json)
- [Arch source-material manifest](SOURCE-MATERIALS-arch.json)
- [CI validation summary](CI-VALIDATION.md)
- [Public source provenance](PUBLIC-SOURCE-PROVENANCE.json)
- [Package checksums](SHA256SUMS.txt)

The receipts identify the approved package source commit and package hashes.
Their sources fields record source archives used inside the private CI
assembly; those private intermediate archives are intentionally not release
assets. The public tag and the runtime source archives linked above provide
the corresponding public source, and the provenance record lists the source
relationship and intentional exclusions. The validation summary distinguishes
package assembly/ABI checks from the separate installation and loader checks.
The release does not claim a general bit-perfect result or universal
compatibility with every distribution, Subsonic server or audio device.

## License and source availability

Sonante's GPL-3.0-only corresponding source includes the source needed to
build, install and run the application, including its scripts. The v0.7.2
public tag and generated source archive provide that source. The separately
bundled runtime source archives remain publicly available at the links above,
with their original license files and source-material records. See the
[GNU GPL version 3](https://www.gnu.org/licenses/gpl-3.0.html) and the license
and notices included in each source package.
