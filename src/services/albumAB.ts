import { invoke } from '@tauri-apps/api/core';
import type { EditionEntry } from '../utils/albumEditions';
import { localSourceReadQueue } from '../utils/localSourceReadQueue';
import { decodeABSample, decodeABTracks, type ABSide, type ABSeconds } from '../utils/albumAB';
const request = (entry: EditionEntry) => entry.source === 'local' ? { kind: 'local', album_id: entry.album.id }
  : entry.source === 'plex' ? { kind: 'plex', server_id: entry.serverId, library_id: entry.libraryId, album_id: entry.album.rating_key }
  : { kind: 'navidrome', account_id: entry.accountId, connection_id: entry.connectionId, album_id: entry.album.id };
const read = <T>(entry: EditionEntry, current: () => boolean, work: () => Promise<T>): Promise<T> => {
  const guarded = () => { if (!current()) throw 'AB_CHANGED'; return work(); };
  return entry.source === 'local' ? localSourceReadQueue(guarded) : Promise.resolve().then(guarded);
};
export const albumABService = {
  tracks: (entry: EditionEntry, current: () => boolean) => read(entry, current, async () => { const raw = await invoke<unknown>('ab_tracks', { source: request(entry) }); if (!current()) throw 'AB_CHANGED'; return decodeABTracks(raw); }),
  begin: () => invoke<string>('ab_begin'),
  async sample(token: string, side: ABSide, entry: EditionEntry, trackKey: string, start: number, seconds: ABSeconds, current: () => boolean) {
    // Queue only the short Local admission/open, never the decoder. Transport
    // controls can run while the separate bounded decoder worker prepares PCM.
    await read(entry, current, () => invoke<void>('ab_load', { token, side, source: request(entry), trackKey }));
    if (!current()) throw 'AB_CHANGED';
    const raw = await invoke<ArrayBuffer>('ab_decode', { token, side, start, seconds });
    if (!current()) throw 'AB_CHANGED'; return decodeABSample(raw);
  },
  start: (token: string, intent: string) => invoke<void>('ab_start', { token, intent }),
  alive: (token: string, intent: string) => invoke<boolean>('ab_alive', { token, intent }),
  end: (token: string, intent: string) => invoke<void>('ab_end', { token, intent }),
  cancel: (token: string) => invoke<void>('ab_cancel', { token }),
};
