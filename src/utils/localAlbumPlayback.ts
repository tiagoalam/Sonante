import { beginPlaybackIntent, isPlaybackSuperseded, type PlaybackIntent } from "./playbackIntent.ts";
import type { LocalAlbum } from "../types/local";
import type { TrackMetadata } from "../types/audio";
import { flattenAlbumDiscs, type AlbumDiscTracks } from "./localAlbumDiscs.ts";

// Each new selection invalidates prior preparation across all playback views.
export function createLocalAlbumPlayback(
  load: (id: string) => Promise<AlbumDiscTracks[]>,
  cover: (album: LocalAlbum) => Promise<string | null>,
  play: (tracks: TrackMetadata[], index: number, intent?: PlaybackIntent) => Promise<void>,
) {
  let generation = 0; let active: PlaybackIntent | undefined;
  return {
    cancel() { generation++; active?.cancel(); },
    async play(album: LocalAlbum, current: () => boolean): Promise<void> {
      if (!current()) return;
      const intent = beginPlaybackIntent(); active = intent;
      const request = generation;
      const valid = () => request === generation && current() && intent.current();
      try {
        const files = flattenAlbumDiscs(await load(album.id)).tracks;
        if (!valid() || !files.length) return;
        const thumb = await cover(album);
        if (!valid()) return;
        // Same ordering, URI and metadata fallbacks as LocalBrowser's album action.
        await play(files.map(file => ({ title: file.title || file.name,
          artist: file.artist || album.artist, album: file.album || album.title,
          thumb, uri: file.path, duration: file.duration })), 0, intent);
      } catch (error) { if (valid() && !isPlaybackSuperseded(error) && await intent.confirmCurrent()) throw error; }
    },
  };
}
