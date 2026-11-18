# Complete Linux package assembly

The manual `Build complete Linux packages` workflow accepts one full 40-character commit SHA. Ubuntu, Fedora-profile and Arch jobs explicitly check out that SHA, verify that `HEAD` is exactly the requested commit, and verify that it belongs to the selected workflow ref. Full history is fetched for this ancestry check. The complete-package job repeats the same source gate before assembling artifacts. A commit hash in an artifact is provenance, not a substitute for the checkout equality check.

After all package inputs pass, `scripts/complete-packages.py` assembles **complete** DEB, Fedora-profile RPM and native Arch installers. The final artifact is named `sonante-0.7.0-candidate-<full-commit-sha>`. Intermediate `sonante-compilation-*-inputs-<sha>` artifacts are not complete installers. The workflow has read-only repository permissions and does not create a tag, GitHub Release or public release assets.

The pinned baseline is the publicly released 0.5.5 revision 10. `complete-packages.json` records exact installer URLs, sizes and SHA256 values, compiled runtime descriptors and corresponding-source sidecars. Only its runtime libraries, resources, license inventory and established distribution dependencies are reused. The application comes from the current workflow's compiled inputs; their checksums and source-commit records must match the clean checkout. Current application icons, desktop entry and notices accompany it. Runtime pins are unchanged. This does not claim byte-reproducible engine builds.

The assembler rejects changed downloads, mismatched descriptors, missing or extra runtime resources, archive traversal, duplicates, links, special files, writable payloads, installer scripts and unreviewed application library dependencies. Ubuntu/Fedora objects retain GLIBC 2.35, GLIBCXX 3.4.30 and CXXABI 1.3.13 ceilings. The Arch application is capped at GLIBC 2.43, and its exact lower requirement is written into package metadata. The Arch binary must use the OpenSSL 3 SONAME pair and remain within the OpenSSL 3.5.9 symbol-version ceiling; OpenSSL 4 is rejected. Fedora retains its own descriptor and system Nettle requirement. Encoded packages are reread to compare payload hashes, sizes, modes and metadata; RPM header digests must also agree.

## Arch ABI build and compatibility policy

The Arch CI job uses the signed Arch Linux Archive snapshot `2026/07/24` in its disposable container. It is pinned at the last GLIBC 2.43 baseline before Arch moved to GLIBC 2.44 on 2026-07-25. It retains the runner image's current trusted keyring instead of downgrading `archlinux-keyring`, whose snapshot package hook requires a secret signing key unavailable in the disposable container. The setup script keeps pacman signature checking enabled, rejects leftover packages that do not belong to that snapshot, and asserts GLIBC 2.43 and the snapshot's OpenSSL 3.6 build line before building. The resulting executable is checked for OpenSSL symbol compatibility, then the final package is loaded against both current Arch `openssl-3` and the exact signed OpenSSL 3.6.5 libraries used by Manjaro Stable. `makepkg` still disables LTO with `!lto`; Rust LTO remains enabled. The workflow then inspects the compiled ELF and installs the package into the same disposable build environment.

The application is linked against OpenSSL 3.x and the final ELF is tested by the loader against both the 3.5.9 compatibility runtime and the 3.6.5 runtime. The package depends on `libcrypto.so=3-64` and `libssl.so=3-64`, not a package literally named `openssl`. This lets Manjaro Stable's OpenSSL 3.6.5 package and current Arch's `openssl-3` compatibility package satisfy the same SONAME requirements while current Arch may keep OpenSSL 4 as its primary package. CI tests the final package with pacman on current Arch and checks dynamic loading against the signed OpenSSL 3.6.5 libraries from the official Arch archive, on a GLIBC 2.44 environment. This is an ABI-equivalent Manjaro check, not a full Manjaro installation test.

The build snapshot is deliberately not advanced automatically: moving it can raise the ELF floor or change the OpenSSL symbols. Distro-provided runtime libraries continue receiving the user's system security updates because Sonante does not bundle OpenSSL. Refreshing the build snapshot or toolchain is a reviewed packaging change; it must pass the same ABI ceilings and target-environment checks. If a security fix to build tooling cannot be incorporated without violating the supported baseline, release preparation must stop for an explicit compatibility decision rather than silently raising the minimum.

For the 0.7.0 candidate, the complete artifact includes:

- `sonante_0.7.0-1_amd64.deb`, `sonante-0.7.0-1.x86_64.rpm` and `sonante-0.7.0-1-x86_64.pkg.tar.zst`.
- Standard and Fedora-overlay application source archives from the exact clean Git commit.
- The two checksum-pinned runtime corresponding-source archives, as separate downloads.
- `BUILD-RECEIPT.json`, which records the common commit and source tree, package and application hashes, runtime descriptor identity and validation scope.
- `SHA256SUMS.txt` covering the packages, source archives, receipts and included source sidecars.

The Ubuntu, Fedora and Arch build jobs emit and checksum `source-commit.txt`. Assembly rejects any input whose source commit differs from the clean checkout used to produce the final artifacts. This ensures the three installers identify the same candidate commit; it does not claim byte-reproducible builds.

No runtime is downloaded when Sonante starts. Packages contain Sonante's pinned private MPD runtimes; they do not require or start a system MPD daemon. Assembly never installs a package or executes Sonante, MPD or CamillaDSP. Creating tags and publishing release assets are separate authorized operations. Passing these checks does not replace representative installation and audio validation.

To prepare a candidate in GitHub Actions, first make the commit available on the selected branch and ensure this workflow is present on the repository's default branch. Then open **Actions → Build complete Linux packages → Run workflow**, select the branch containing the candidate and enter the exact full SHA of the candidate commit. Download only the final `sonante-0.7.0-candidate-<sha>` artifact; intermediate compilation artifacts are not installers. This repository is public, so Actions artifacts should not be treated as private; use only approved candidate content and observe the configured retention period.

## Local assembly

Use a clean committed checkout matching the workflow artifacts. The assembler supports Ubuntu 22.04 packaging tools; compression uses `DPKG_DEB_THREADS_MAX` rather than the newer [dpkg CLI thread option](https://manpages.debian.org/bookworm/dpkg/dpkg-deb.1.en.html). Install packaging tools (`python3` 3.10+, `binutils`, `dpkg-deb`, RPM including `rpmbuild`, `bsdtar`, and `zstd`). Extract the three compilation artifacts into sibling directories named `ubuntu`, `fedora` and `arch`, preserving their checksum and source-commit files.

```bash
python3 scripts/complete-packages.py \
  --candidates /absolute/path/to/candidates \
  --work-dir /absolute/path/to/new-work-directory \
  --output /absolute/path/to/new-output-directory
```

Work and output directories must be fresh and outside the source checkout. Failed attempts are retained. The assembler fetches only the pinned public baseline inputs and verifies their bytes before opening them. `--revision` can select a later positive packaging revision for the same application source; it never changes the source version.
