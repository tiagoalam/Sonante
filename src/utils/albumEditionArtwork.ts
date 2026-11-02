import type { EditionEntry } from './albumEditions';
import type { LocalAlbum } from '../types/local';
import type { PlexImageRef } from '../types/plex';

export type EditionArtworkCandidate =
  | { kind: 'plex'; key: string; image: PlexImageRef }
  | { kind: 'local'; key: string; album: LocalAlbum }
  | { kind: 'navidrome'; key: string; connectionId: string; coverId: string };

// Copies here are the measured occurrences of ONE edition. Never borrow an
// image from another edition just because it belongs to the same work.
export function editionArtworkCandidates(entry: EditionEntry, copies: readonly EditionEntry[] = [entry], coverId?: string | null): EditionArtworkCandidate[] {
  const result: EditionArtworkCandidate[] = [], seen = new Set<string>();
  for (const copy of [entry, ...copies]) {
    let candidate: EditionArtworkCandidate | null = null;
    if (copy.source === 'plex' && copy.album.thumb) {
      candidate = { kind: 'plex', key: JSON.stringify(['plex', copy.album.thumb.server_id, copy.album.thumb.path]), image: copy.album.thumb };
    } else if (copy.source === 'local') {
      candidate = { kind: 'local', key: JSON.stringify(['local', copy.album.id, copy.album.source_id, copy.album.folder_path, copy.album.discs.map(disc => disc.folder_path)]), album: copy.album };
    } else if (copy.source === 'navidrome') {
      const id = (copy.key === entry.key ? coverId : null) || copy.album.cover_id;
      if (id) candidate = { kind: 'navidrome', key: JSON.stringify(['navidrome', copy.connectionId, id]), connectionId: copy.connectionId, coverId: id };
    }
    if (candidate && !seen.has(candidate.key)) { seen.add(candidate.key); result.push(candidate); }
  }
  const rank = { plex: 0, local: 1, navidrome: 2 };
  return result.sort((a, b) => rank[a.kind] - rank[b.kind]);
}
