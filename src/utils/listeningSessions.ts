import type { CurrentMedia, MediaLocator, TrackMetadata } from '../types/audio';
import type { ListeningSession, SessionAlbum, SessionCandidate, SessionMode } from '../types/listeningSession';
import type { SmartSource } from '../types/smartPlaylist';
export const SESSION_KEY = 'sonante.listeningSessions.v1';
export const SESSION_EVENT = 'sonante-listening-sessions-changed';
export const MAX_SESSIONS = 24;
export function sessionTrackKey(locator: MediaLocator | CurrentMedia | null | undefined): string | null {
  if (!locator) return null;
  if (locator.kind === 'local') return JSON.stringify(['local', locator.uri]);
  if (locator.kind === 'plex') return JSON.stringify(['plex', locator.server_id, locator.part_key]);
  if (locator.kind === 'navidrome') return JSON.stringify(['navidrome', 'account_id' in locator ? locator.account_id : null, locator.song_id]);
  return null;
}
export const sessionKeys = (session: ListeningSession) => session.albums.flatMap(album => album.keys);
export function trackBelongsToSource(track: TrackMetadata, source: SmartSource): boolean {
  const l = track.media_locator;
  return !!l && (source.kind === 'local' ? l.kind === 'local' : source.kind === 'plex'
    ? l.kind === 'plex' && l.server_id === source.server_id
    : l.kind === 'navidrome' && l.account_id === source.account_id);
}
export function sessionAlbum(album: SessionCandidate, tracks: TrackMetadata[], source: SmartSource, reason: SessionAlbum['reason']): SessionAlbum {
  if (!tracks.length || tracks.length > 500 || tracks.some(track => !trackBelongsToSource(track, source))) throw 'SESSION_READ';
  const keys = tracks.map(track => sessionTrackKey(track.media_locator)!);
  if (new Set(keys).size !== keys.length) throw 'SESSION_CHANGED';
  const duration = tracks.every(track => Number.isFinite(track.duration) && track.duration! > 0)
    ? tracks.reduce((sum, track) => sum + track.duration!, 0) : null;
  return { id: album.id, title: album.title, artist: album.artist, year: album.year, duration, keys, reason: reason === 'recent' && album.added_at === null ? 'discovery' : reason };
}
export function orderedCandidates(albums: SessionCandidate[], mode: SessionMode, random = Math.random): SessionCandidate[] {
  const values = [...albums];
  for (let i = values.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [values[i], values[j]] = [values[j], values[i]]; }
  if (mode === 'recent') values.sort((a, b) => (b.added_at ?? -1) - (a.added_at ?? -1));
  return values;
}
// Compare a small pool of complete albums. Candidate order remains the session
// order; duration only selects a subset, never clips or reorders its tracks.
function closestSessionSelection(pool: { album: SessionAlbum; tracks: TrackMetadata[] }[], target: number): number[] {
  let best: number[] = [], bestDuration = 0, difference = Infinity;
  const selected: number[] = [];
  const visit = (from: number, duration: number, tracks: number) => {
    for (let index = from; index < pool.length; index++) {
      const item = pool[index], totalTracks = tracks + item.tracks.length;
      if (item.album.duration === null || totalTracks > 1500) continue;
      const total = duration + item.album.duration, error = Math.abs(total - target);
      selected.push(index);
      // Equal distances prefer a complete session above the target rather than
      // systematically producing the shorter option (e.g. 40 vs 80 for 60).
      if (error < difference || error === difference && total >= target && bestDuration < target) {
        best = [...selected]; bestDuration = total; difference = error;
      }
      if (selected.length < 6 && total < target) visit(index + 1, total, totalTracks);
      selected.pop();
    }
  };
  visit(0, 0, 0); return best;
}
// At most 24 source reads (also bounds history filtering), eight eligible albums
// and six selected albums. Stop early within 5% or one minute of the target.
export async function prepareSessionAlbums(candidates: SessionCandidate[], source: SmartSource, target: number, mode: SessionMode,
  read: (album: SessionCandidate) => Promise<TrackMetadata[]>, current: () => boolean,
  heard: ReadonlySet<string> = new Set()): Promise<{ albums: SessionAlbum[]; tracks: TrackMetadata[][] }> {
  const pool: { album: SessionAlbum; tracks: TrackMetadata[] }[] = [];
  let selected: number[] = [];
  for (const candidate of candidates.slice(0, 24)) {
    if (!current()) throw 'SESSION_CANCELLED';
    const tracks = await read(candidate);
    if (!current()) throw 'SESSION_CANCELLED';
    const album = sessionAlbum(candidate, tracks, source, mode);
    if (mode === 'unplayed' && album.keys.some(key => heard.has(key))) continue;
    // Preserve an honest unknown-duration fallback; do not pretend it fills a
    // timed gap or mix it into an otherwise measurable selection.
    if (album.duration === null) {
      if (!pool.length) return { albums: [album], tracks: [tracks] };
      continue;
    }
    pool.push({ album, tracks }); selected = closestSessionSelection(pool, target);
    const duration = selected.reduce((sum, index) => sum + pool[index].album.duration!, 0);
    if (Math.abs(duration - target) <= Math.max(60, target * 0.05) || pool.length >= 8) break;
  }
  if (!current()) throw 'SESSION_CANCELLED';
  if (!selected.length) throw 'SESSION_EMPTY';
  return { albums: selected.map(index => pool[index].album), tracks: selected.map(index => pool[index].tracks) };
}

type SessionStorage = Pick<Storage, 'getItem' | 'setItem'>;
const text = (value: unknown, max = 512): value is string => typeof value === 'string' && value.length > 0 && value.length <= max && !/[\0\r\n]/.test(value);
function validSource(value: unknown): value is SmartSource {
  if (!value || typeof value !== 'object') return false;
  const source = value as SmartSource;
  return source.kind === 'local' || source.kind === 'plex' && text(source.server_id) && /^\d{1,32}$/.test(source.library_id)
    || source.kind === 'navidrome' && /^[a-f0-9]{64}$/.test(source.account_id);
}
export function validSession(value: unknown): value is ListeningSession {
  if (!value || typeof value !== 'object') return false;
  const s = value as ListeningSession;
  return text(s.id) && text(s.name, 120) && Number.isSafeInteger(s.created_at) && s.created_at > 0 && validSource(s.source)
    && (s.started_at === undefined || Number.isSafeInteger(s.started_at) && s.started_at > 0)
    && [1800, 3600, 5400].includes(s.target) && typeof s.completed === 'boolean'
    && Array.isArray(s.albums) && s.albums.length > 0 && s.albums.length <= 6
    && s.albums.every(a => a && text(a.id) && text(a.title) && typeof a.artist === 'string' && a.artist.length <= 512
      && (a.year === null || Number.isInteger(a.year)) && (a.duration === null || Number.isFinite(a.duration) && a.duration > 0)
      && ['discovery', 'recent', 'unplayed', 'selected'].includes(a.reason) && Array.isArray(a.keys) && a.keys.length > 0 && a.keys.length <= 500
      && a.keys.every(key => text(key, 16384)))
    && sessionKeys(s).length <= 1500 && Number.isInteger(s.cursor) && s.cursor >= 0 && s.cursor < sessionKeys(s).length;
}
export function readSessions(storage: SessionStorage): ListeningSession[] {
  const raw = storage.getItem(SESSION_KEY);
  if (!raw) return [];
  if (raw.length > 1_500_000) throw 'SESSION_STORAGE';
  const value = JSON.parse(raw);
  if (value?.version !== 1 || !Array.isArray(value.sessions) || value.sessions.length > MAX_SESSIONS
    || !value.sessions.every(validSession) || new Set(value.sessions.map((s: ListeningSession) => s.id)).size !== value.sessions.length) throw 'SESSION_STORAGE';
  return value.sessions;
}
export function writeSessions(storage: SessionStorage, sessions: ListeningSession[]): void {
  if (sessions.length > MAX_SESSIONS || !sessions.every(validSession) || new Set(sessions.map(s => s.id)).size !== sessions.length) throw 'SESSION_STORAGE';
  const raw = JSON.stringify({ version: 1, sessions });
  if (raw.length > 1_500_000) throw 'SESSION_STORAGE';
  storage.setItem(SESSION_KEY, raw);
}
