# Complete Linux package assembly

CI builds the checked-out source for Ubuntu 22.04, the Fedora runtime profile and native Arch. After both build jobs succeed, `scripts/complete-packages.py` assembles **complete** installers and uploads `sonante-complete-linux-packages`. Intermediate `sonante-compilation-*-inputs` artifacts are not complete installers.

The pinned baseline is the publicly released 0.5.5 revision 10. `complete-packages.json` records exact installer URLs, sizes and SHA256 values, compiled runtime descriptors and corresponding-source sidecars. Only its runtime libraries, resources, license inventory and established distribution dependencies are reused. The application comes from the current workflow's compiled inputs; their checksums and source-commit records must match the clean checkout. Current application icons, desktop entry and notices accompany it. Runtime pins are unchanged. This does not claim byte-reproducible engine builds.

The assembler rejects changed downloads, mismatched descriptors, missing or extra runtime resources, archive traversal, duplicates, links, special files, writable payloads, installer scripts and unreviewed application library dependencies. Ubuntu/Fedora objects retain GLIBC 2.35, GLIBCXX 3.4.30 and CXXABI 1.3.13 ceilings. The native Arch application's actual glibc requirement is written into its package metadata. Fedora retains its own descriptor and system Nettle requirement. Encoded packages are reread to compare payload hashes, sizes, modes and metadata; RPM header digests must also agree.

The complete artifact includes:

- DEB, Fedora-profile RPM and native Arch installers for the current application version and the package revision selected by the assembly command (revision 1 for 0.5.7 in CI).
- Standard and Fedora-overlay application source archives from the exact clean Git commit.
- The two checksum-pinned runtime corresponding-source archives, as separate downloads.
- `BUILD-RECEIPT.json` and `SHA256SUMS.txt` covering the package/source outputs.

No runtime is downloaded when Sonante starts. Assembly never installs a package or executes Sonante, MPD or CamillaDSP. Workflow permissions remain `contents: read`; creating tags and publishing release assets are separate authorized operations. Passing these static checks does not replace representative installation and audio validation.

## Local assembly

Use a clean committed checkout matching the workflow artifacts. The assembler supports Ubuntu 22.04 packaging tools; compression uses `DPKG_DEB_THREADS_MAX` rather than the newer [dpkg CLI thread option](https://manpages.debian.org/bookworm/dpkg/dpkg-deb.1.en.html). Install packaging tools (`python3` 3.10+, `binutils`, `dpkg-deb`, RPM including `rpmbuild`, `bsdtar`, and `zstd`). Extract the three compilation artifacts into sibling directories named `ubuntu`, `fedora` and `arch`, preserving their checksum and source-commit files.

```bash
python3 scripts/complete-packages.py \
  --candidates /absolute/path/to/candidates \
  --work-dir /absolute/path/to/new-work-directory \
  --output /absolute/path/to/new-output-directory
```

Work and output directories must be fresh and outside the source checkout. Failed attempts are retained. The assembler fetches only the pinned public baseline inputs and verifies their bytes before opening them. `--revision` can select a later positive packaging revision for the same application source; it never changes the source version.
