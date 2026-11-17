# Sonante 0.7.0 — homologation candidate

**Not published. No installable 0.7.0 packages or public tag are available yet.** These notes describe the current candidate, not a release announcement.

Sonante 0.7.0 prepares the Local library for a persistent SQLite catalog while retaining the existing player, supported audio routes and Plex integration.

- Local sources are registered and indexed into SQLite. The library opens from its saved catalog without automatically rescanning folders.
- Users upgrading from a public installation can manually import previously saved Local folders from the Local library screen. This indexes the configured folders; it does not move, rename or edit music files.
- Old configuration, favorites and queue data are preserved. A Local favorite without an exact safe match may need to be added again. The old queue is not automatically migrated to SQLite locators.
- Shared, Direct and EQ continue to use the existing playback engine and route validation. Reported format information is not a measurement at the DAC; Sonante makes no bit-perfect guarantee.
- The optional A/B comparison supports content-based automatic alignment when a confident match is found, with manual timing adjustment retained as fallback.
- DSF DSD64 mono/stereo samples can be compared after conversion to PCM. This is a comparison-only path, not native DSD playback; other DSF rates or channel layouts are outside this support.
- Library navigation, metadata diagnostics, tag editing, artwork, search and playback lifecycle received updates. Tag writing remains limited to supported physical FLAC files and per-file results.

The candidate is not yet qualified for public distribution. Linux package assembly and installation validation remain for RELEASE-CI-01. The Windows development tree has not been qualified as a 0.7.0 release target.

## Previous public release

The previous published release was Sonante 0.6.3. Its notes are retained below for historical reference.

---

# Sonante 0.6.3

Sonante 0.6.3 focuses on clearer playback feedback and more predictable library workflows.

- Smart-playlist previews now vary album and track selection, with an explicit way to generate another preview without changing the queue.
- The Albums view shows compact online-artwork enrichment progress without shifting the catalog layout.
- Track and queue selection is available on demand, preserving normal playback and queue reordering.
- Current MPD playback errors are visible in the player interface; stale errors remain suppressed after successful playback.
- Signal Path retains factual route, format, and observed-output information and no longer labels playback as bit-perfect.
- ALSA Direct uses the compatibility setting validated with the FiiO K17 and Astro A50. This validation does not certify bit-perfect output.

Linux packages are provided for Ubuntu/Debian, Fedora, and Arch. Verify package checksums before installation.
