import type { LocalAlbum } from "../types/local";
import type { TrackMetadata } from "../types/audio";
import { flattenAlbumDiscs, type AlbumDiscTracks } from "./localAlbumDiscs.ts";

// One admission across Explore shelves/slices; invalidation never releases an in-flight read.
export function createLocalAlbumPlayback(
  load: (id: string) => Promise<AlbumDiscTracks[]>,
  cover: (album: LocalAlbum) => Promise<string | null>,
  play: (tracks: TrackMetadata[], index: number) => Promise<void>,
) {
  let pending = false, generation = 0;
  return {
    cancel() { generation++; },
    async play(album: LocalAlbum, current: () => boolean): Promise<void> {
      if (pending || !current()) return;
      pending = true;
      const request = generation;
      const valid = () => request === generation && current();
      try {
        const files = flattenAlbumDiscs(await load(album.id)).tracks;
        if (!valid() || !files.length) return;
        const thumb = await cover(album);
        if (!valid()) return;
        // Same ordering, URI and metadata fallbacks as LocalBrowser's album action.
        await play(files.map(file => ({ title: file.title || file.name,
          artist: file.artist || album.artist, album: file.album || album.title,
          thumb, uri: file.path, duration: file.duration })), 0);
      } finally { pending = false; }
    },
  };
}
