import type { ListeningSession } from '../types/listeningSession';
import type { PlaybackStatus, TrackMetadata } from '../types/audio';
import { audioService } from '../services/audio';
import { listeningSessionStore } from './listeningSessionStore';
import { sessionKeys, sessionTrackKey } from './listeningSessions';
// Dormant until an explicit session play. Draft progress lives only in memory;
// persistence can update an existing explicit save, but cannot create one.
let binding: { session: ListeningSession; revision: string; tracks: TrackMetadata[]; cursor: number; generation: number } | null = null;
let playback: ListeningSession | null = null;
const listeners = new Set<() => void>();
export const subscribeSessionPlayback = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const getSessionPlayback = () => playback;
function publish(session: ListeningSession | null) { playback = session; for (const listener of listeners) listener(); }
let generation = 0, pending = false, retryAt = 0;
export function suspendSessionProgress() { binding = null; generation++; if (playback) publish(null); }
export async function bindSessionProgress(session: ListeningSession, current: () => boolean): Promise<void> {
  const own = ++generation; binding = null; retryAt = 0; if (playback) publish(null);
  const snapshot = await audioService.getQueueEditSnapshot();
  if (!current() || own !== generation) return;
  const keys = sessionKeys(session);
  if (JSON.stringify(snapshot.tracks.map(track => sessionTrackKey(track.media_locator))) !== JSON.stringify(keys)) throw 'SESSION_CHANGED';
  binding = { session, revision: snapshot.revision, tracks: snapshot.tracks, cursor: -1, generation: own };
  publish(session);
  listeningSessionStore.updateProgress(session, true);
}
function observedMatches(status: PlaybackStatus, track: TrackMetadata | undefined): boolean {
  const media = status.current_media, locator = track?.media_locator;
  if (!media || !locator || media.kind !== locator.kind) return false;
  return media.kind === 'navidrome' && locator.kind === 'navidrome'
    ? media.connection_id === locator.connection_id && media.song_id === locator.song_id
    : sessionTrackKey(media) === sessionTrackKey(locator);
}
export async function observeSessionProgress(status: PlaybackStatus, now = Date.now()): Promise<void> {
  const own = binding, index = status.current_media?.queue_index;
  if (!own || pending || now < retryAt || index === undefined || status.snapshot_consistent === false
    || !['play', 'pause'].includes(status.state)) return;
  if (index === own.cursor && observedMatches(status, own.tracks[index])) return;
  pending = true;
  try {
    let snapshot;
    try { snapshot = await audioService.getQueueEditSnapshot(); }
    catch { retryAt = now + 5000; return; }
    if (binding !== own || own.generation !== generation) return;
    if (snapshot.revision !== own.revision) { suspendSessionProgress(); return; }
    // A poll started before Play can arrive after the new queue was committed.
    if (!Number.isInteger(index) || index < 0 || index >= own.tracks.length || !observedMatches(status, snapshot.tracks[index])) return;
    const value = { ...own.session, cursor: index };
    own.session = value; own.cursor = index; publish(value);
    listeningSessionStore.updateProgress(value);
  } catch { suspendSessionProgress(); } // Keep the last saved position; never change audio or clear data.
  finally { pending = false; }
}
