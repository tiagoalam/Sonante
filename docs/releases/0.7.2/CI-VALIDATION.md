# CI validation record

The private package workflow run 45475585333 built the three candidates from
source commit ee3eca800cac5bf2bb0773ed933bbfcbe9d36b0a, version 0.7.2.

| Platform | Build and assembly | Compatibility check |
| --- | --- | --- |
| Ubuntu 22.04 | DEB compiled and assembled | Installed; loader checked |
| Fedora 44 | RPM compiled and assembled | Installed; loader checked |
| Arch Linux | Built against the signed 2026-07-24 snapshot; package assembled | Installed on current Arch with OpenSSL 3; loader and ABI checked |

The build receipts record each package identity, hash, runtime descriptor and
ABI inspection. The compatibility job separately installed the completed
packages and checked their dynamic loaders. The Arch build used the pinned
GLIBC 2.43-compatible snapshot and the OpenSSL 3 bootstrap transition.

The release package workflow did not test audio output on physical hardware.
Manual use of the 0.7.2 package on Manjaro was reported separately, including
WavPack playback and the carousel keyboard behavior. No general bit-perfect
claim is made.
