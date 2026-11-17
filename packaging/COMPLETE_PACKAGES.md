# Complete Linux package assembly

The manual `Build complete Linux packages` workflow accepts one full 40-character commit SHA. Ubuntu, Fedora-profile and Arch jobs explicitly check out that SHA, verify that `HEAD` is exactly the requested commit, and verify that it belongs to the selected workflow ref. Full history is fetched for this ancestry check. The complete-package job repeats the same source gate before assembling artifacts. A commit hash in an artifact is provenance, not a substitute for the checkout equality check.

After all package inputs pass, `scripts/complete-packages.py` assembles **complete** DEB, Fedora-profile RPM and native Arch installers. The final artifact is named `sonante-0.7.0-candidate-<full-commit-sha>`. Intermediate `sonante-compilation-*-inputs-<sha>` artifacts are not complete installers. The workflow has read-only repository permissions and does not create a tag, GitHub Release or public release assets.

The pinned baseline is the publicly released 0.5.5 revision 10. `complete-packages.json` records exact installer URLs, sizes and SHA256 values, compiled runtime descriptors and corresponding-source sidecars. Only its runtime libraries, resources, license inventory and established distribution dependencies are reused. The application comes from the current workflow's compiled inputs; their checksums and source-commit records must match the clean checkout. Current application icons, desktop entry and notices accompany it. Runtime pins are unchanged. This does not claim byte-reproducible engine builds.

The assembler rejects changed downloads, mismatched descriptors, missing or extra runtime resources, archive traversal, duplicates, links, special files, writable payloads, installer scripts and unreviewed application library dependencies. Ubuntu/Fedora objects retain GLIBC 2.35, GLIBCXX 3.4.30 and CXXABI 1.3.13 ceilings. The native Arch application's actual glibc requirement is written into its package metadata. Fedora retains its own descriptor and system Nettle requirement. Encoded packages are reread to compare payload hashes, sizes, modes and metadata; RPM header digests must also agree.

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
