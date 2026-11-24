# Complete Linux packages

Sonante 0.7.2 is distributed as complete DEB, RPM and Arch packages. Each
installer contains the application and its pinned MPD runtime. The packages do
not require or start a system MPD service.

## Public release downloads

The uploaded release assets are exactly:

- sonante_0.7.2-1_amd64.deb
- sonante-0.7.2-1.x86_64.rpm
- sonante-0.7.2-1-x86_64.pkg.tar.zst
- SHA256SUMS.txt

The checksum file covers those three package files only. Build receipts, source
SOURCE-MATERIALS-*.json manifests, CI validation and source provenance are versioned under
docs/releases/0.7.2. The public source tree is available from the v0.7.2 tag.
The release source guide links to the corresponding source archives for the
bundled runtimes.

## Corresponding runtime sources

The installers reuse pinned MPD and support-library runtimes. Platform source
material manifests identify their corresponding public source archives by
filename, byte length and SHA-256. They remain available from the public
Sonante 0.5.5 source release. The Fedora manifest also identifies its
compatibility source archive. These archives are not installed by the package.

Verify the downloaded packages before installation:

    sha256sum -c SHA256SUMS.txt

The Ubuntu package uses the system-provided Nettle library. Arch and Fedora
retain their qualified runtime profiles. See DEB-SYSTEM-NETTLE.md for the
Debian/Ubuntu dependency details.

## Package assembly

scripts/complete-packages.py assembles complete packages from checksum-verified
platform build inputs and pinned runtime baselines. The build receipts identify
the exact source commit and package hashes; they do not claim byte-reproducible
engine builds.

The v0.7.2 packages passed the project package and installation/loader checks
on Ubuntu 22.04, Fedora 44 and current Arch with OpenSSL 3 compatibility. A
Manjaro installation was also manually exercised. These checks do not replace
distribution-specific or physical audio validation.
