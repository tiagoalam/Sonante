CamillaDSP 4.1.3 — Sonante Windows private PCM / WASAPI candidate2

This package contains the complete corresponding source and vendored crates,
unchanged Cargo manifest/lock/default features/release profile, original licenses,
Windows-only adapter patch, source hashes, audit and observed build receipt.
The published source materials and upstream commit in SOURCES.json were verified
before this source was adapted. All 358 vendored packages / 21561 declared file
checksums were verified, and all vendor bytes match that audited base.

The source tree is the final PATCHED build source. Do not apply the patch again.
Patch before/after hashes record the original source changes for review.
DSP/filter/sample conversion and Windows capture implementation remain unchanged.
WASAPI render now retains full-ring chunks or reports bounded backpressure, and
acknowledges EOF only after queued frames/client padding drain and stop_stream.
These checks do not measure a DAC, prove native WASAPI, or claim bit-perfect.

From a fresh extracted directory:
  python3 rebuild.py --verify-only
  python3 rebuild.py --build

Cross-build prerequisites on Linux x86_64: Rust/Cargo and MSVC target, cargo-xwin,
LLVM/lld and verified Windows SDK/CRT cache as in the original cross build. The
original build used task-local Rust1.99 and static CRT, nice10/j1. No dependency
network fetch is needed by Cargo because vendor/lock/config are included. XWIN
SDK/CRT toolchain materials must already be available/verified. This recipe does
not execute any Windows program or start playback. Source verification does not
claim byte-for-byte binary reproducibility.

Original project license choice GPL-3.0-only OR MPL-2.0; original texts in source/
LICENSE_GLPv3.txt and LICENSE_MPL2.0.txt (original filenames). All third party
licenses remain in source/vendor. The installer additionally includes those
original license/notice files. No user media, credentials or profiles included.
