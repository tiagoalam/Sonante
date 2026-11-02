import type { ListeningSession, SessionAlbum, SessionCandidate, SessionMode } from '../types/listeningSession';
import type { TrackMetadata } from '../types/audio';
import { orderedCandidates, sessionAlbum } from './listeningSessions';

export function sessionPosition(session: Pick<ListeningSession, 'cursor' | 'albums'> | null) {
  if (!session || !Number.isInteger(session.cursor) || session.cursor < 0) return null;
  const total = session.albums.reduce((sum, album) => sum + album.keys.length, 0);
  if (session.cursor >= total) return null;
  let start = 0;
  for (let index = 0; index < session.albums.length; index++) {
    const count = session.albums[index].keys.length;
    if (session.cursor < start + count) return {
      albumIndex: index, album: index + 1, albums: session.albums.length,
      track: session.cursor - start + 1, tracks: count,
      position: session.cursor + 1, total,
    };
    start += count;
  }
  return null;
}

// Pure catalog metadata work: opening choices never expands tracks or mutates a session.
export function sessionReplacementCandidates(pool: readonly SessionCandidate[], session: ListeningSession, mode: SessionMode, random = Math.random): SessionCandidate[] {
  const seen = new Set(session.albums.map(album => album.id));
  return orderedCandidates(pool.filter(album => {
    if (seen.has(album.id)) return false;
    seen.add(album.id); return true;
  }), mode, random);
}

// Resolve only the explicit choice. The existing caller fences source/account,
// navigation and playback context; cancellation never returns a partial edit.
export async function prepareSessionReplacement(session: ListeningSession, albumId: string, candidate: SessionCandidate,
  read: (album: SessionCandidate) => Promise<TrackMetadata[]>, current: () => boolean): Promise<SessionAlbum[]> {
  if (!current()) throw 'SESSION_CANCELLED';
  const index = session.albums.findIndex(album => album.id === albumId);
  if (index < 0 || session.albums.some(album => album.id === candidate.id)) throw 'SESSION_CHANGED';
  const tracks = await read(candidate);
  if (!current()) throw 'SESSION_CANCELLED';
  const replacement = sessionAlbum(candidate, tracks, session.source, 'selected');
  const albums = session.albums.map((album, i) => i === index ? replacement : album);
  if (albums.reduce((sum, album) => sum + album.keys.length, 0) > 1500) throw 'SESSION_READ';
  return albums;
}
