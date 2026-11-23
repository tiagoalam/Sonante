# Debian and Ubuntu runtime dependency: Nettle

The Debian/Ubuntu package uses the system-provided `libnettle.so.8` for its bundled MPD engines. Its runtime profile omits the private Nettle copy so that the system GnuTLS stack does not load an incompatible duplicate.

The complete DEB declares `libnettle8 | libnettle8t64` to cover distributions using either package name. Other package dependencies and ABI requirements remain in place. The runtime inventory records the system library and package dependency.

Arch and Fedora retain their separate pinned runtime profiles, including their Nettle runtime files. This profile change does not alter the MPD executable, its RPATH, playback code or the other runtime hashes.
