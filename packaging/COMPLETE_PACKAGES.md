# Complete Linux packages

Sonante 0.7.1 is distributed as a complete DEB, RPM and Arch package. Each installer contains the application and its pinned MPD runtime. It does not require or start a system MPD service.

## Release files

The GitHub release includes:

- `sonante_0.7.1-1_amd64.deb`
- `sonante-0.7.1-1.x86_64.rpm`
- `sonante-0.7.1-1-x86_64.pkg.tar.zst`
- `sonante-0.7.1-1-sources.tar.gz` and `sonante-0.7.1-1-fedora-sources.tar.gz`
- Platform build receipts, source-material manifests and `SHA256SUMS.txt`

The standard source archive corresponds to the public `v0.7.1` source snapshot. The Fedora archive also includes the Fedora runtime descriptor. The platform receipts record the build source, package identity, runtime profile and validation scope. The public source manifest describes the relationship between the published source snapshot and the approved Linux package build source.

## Corresponding runtime sources

The installers reuse pinned MPD and support-library runtimes. The platform `SOURCE-MATERIALS-*.json` files identify the matching source archives by public release URL, byte lengths and SHA-256 values. They are available from the public [Sonante 0.5.5 source release](https://github.com/tiagoalam/Sonante/releases/tag/v0.5.5); the Fedora manifest also identifies its compatibility source archive. Those source archives are separate downloads and are not installed by the packages.

Verify downloaded release files before installation:

```sh
sha256sum -c SHA256SUMS.txt
```

The Ubuntu package uses the system-provided Nettle library. Arch and Fedora retain their pinned runtime profiles. See [DEB system Nettle profile](DEB-SYSTEM-NETTLE.md) for the Debian/Ubuntu dependency details.

## Package assembly

`scripts/complete-packages.py` assembles complete packages from checksum-verified platform build inputs and pinned runtime baselines. A clean, committed source checkout is required. The resulting receipts identify the exact source commit and package hashes; they do not claim byte-reproducible engine builds.

The release packages passed package integrity checks and installation/loader checks on Ubuntu 22.04, Fedora 44 and current Arch with OpenSSL 3 compatibility. These checks do not replace distribution-specific or physical audio validation. No physical audio hardware was used for this release.
