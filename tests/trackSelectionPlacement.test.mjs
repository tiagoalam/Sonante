import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(path, import.meta.url), "utf8");

test("Local selection controls are scoped to the opened album track list", () => {
  const local = read("../src/components/LocalBrowserView.tsx");
  assert.doesNotMatch(local, /queueActions\.notice\}\{queueActions\.menu\}\{selection\.toolbar/,
    "the browser-wide header no longer renders the track-selection command");
  assert.match(local, /const selectableTracks = useMemo\(\(\) => selectedAlbum\s*\? albumTracks\.map\(track => localQueueTrack\(track, selectedAlbum, albumCover\)\) : \[\]/,
    "artist/year/genre/grid/folder views do not supply selectable album tracks");
  assert.match(local, /Boolean\(selectedAlbum && tracksReady[\s\S]*?tracksReady && albumTracks\.length > 0 && selection\.toolbar/,
    "the button belongs to the album track header and appears only after its tracks load");
  assert.doesNotMatch(local, /folderSelectionIndices/);
});

test("Plex album and Local favorites details place selection beside their track headings", () => {
  const plex = read("../src/components/AlbumView.tsx");
  const favorites = read("../src/components/FavoritesView.tsx");
  assert.doesNotMatch(plex, /queueActions\.notice\}\{queueActions\.menu\}\{selection\.toolbar/);
  assert.match(plex, /common\.trackTitle[\s\S]*?\{!loading && tracks\.length > 0 && selection\.toolbar\}/);
  assert.doesNotMatch(favorites, /queueActions\.notice\}\{selection\.toolbar/);
  assert.match(favorites, /plex\.tracks[\s\S]*?selectedAlbum && !loadingTracks && tracks\.length > 0 && selection\.toolbar/);
});

test("search results have no album-selection command and Navidrome gains no unsupported selector", () => {
  const search = read("../src/components/UnifiedSearchView.tsx");
  const navidrome = read("../src/components/NavidromeView.tsx");
  assert.doesNotMatch(search, /queueActions\.notice\}\{queueActions\.menu\}\{selection\.toolbar/);
  assert.match(search, /useTrackSelection\(playbackContext,[\s\S]*?, false\)/);
  assert.doesNotMatch(navidrome, /useTrackSelection|selection\.toolbar/);
});

test("artist, playlist and Now Playing track lists do not expose album-only selection", () => {
  for (const file of ["ArtistView.tsx", "PlaylistsView.tsx", "NowPlayingQueue.tsx"]) {
    const source = read(`../src/components/${file}`);
    assert.match(source, /useTrackSelection\([\s\S]*?, false\)/, `${file} disables the shared album selection command`);
    assert.doesNotMatch(source, /\{selection\.toolbar\}/, `${file} has no selection toolbar outside album details`);
  }
});
