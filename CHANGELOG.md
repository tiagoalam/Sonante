# Changelog

## 0.7.0 — homologation candidate (not published)

- Make the Local catalog persistent in SQLite, with explicit source registration, progress reporting, navigation and metadata tools.
- Add a manual import action for Local folders saved by the previous public configuration. Import does not change configuration files or music files; unavailable roots remain pending.
- Preserve old favorites and queue data. Local favorites that cannot be matched by exact identity may need to be added again; the old queue is not automatically rebound to SQLite locators.
- Unify Shared, Direct and EQ audio controls while preserving the existing playback engine and route checks.
- Add content-based automatic sample alignment to the optional A/B comparison, retaining manual timing adjustment when confidence is insufficient.
- Add DSF DSD64 mono/stereo input to A/B comparison through PCM conversion only; this is not native DSD playback and does not establish bit-perfect output.
- Improve library activity, navigation, artwork presentation and playback lifecycle feedback.

## 0.6.3

- Generate more varied smart-playlist previews while preserving filters, limits, and queue state.
- Show compact, real online-artwork enrichment progress in the Albums header.
- Add contextual multi-track selection and improve available-item contrast in navigation.
- Surface current MPD playback errors while suppressing stale errors after successful playback.
- Simplify Signal Path presentation: retain objective route, format, and observed-output details; remove bit-perfect certification.
- Use the ALSA Direct compatibility setting validated on the FiiO K17 and Astro A50. The interface makes no bit-perfect guarantee.
