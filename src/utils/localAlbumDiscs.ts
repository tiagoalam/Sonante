import type { LocalAlbumDisc, LocalItem } from "../types/local";

export interface AlbumDiscTracks {
  disc: LocalAlbumDisc;
  tracks: LocalItem[];
}

export function flattenAlbumDiscs(groups: AlbumDiscTracks[]): {
  tracks: LocalItem[];
  sections: Array<AlbumDiscTracks & { startIndex: number }>;
} {
  const tracks: LocalItem[] = [];
  const sections = groups.map(({ disc, tracks: discTracks }) => {
    const startIndex = tracks.length;
    tracks.push(...discTracks);
    return { disc, tracks: discTracks, startIndex };
  });
  return { tracks, sections };
}
