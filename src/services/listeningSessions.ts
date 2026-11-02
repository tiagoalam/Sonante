import { audioService } from './audio';
import { smartPlaylistsService } from './smartPlaylists';
import { plexService } from './plex';
import { navidromeService } from './navidrome';
import { localSourceReadQueue } from '../utils/localSourceReadQueue';
import { navidromeQueueTrack } from '../utils/navidromeTracks';
import { sessionAlbum } from '../utils/listeningSessions';
import type { SessionCandidate } from '../types/listeningSession';
import type { SmartSource } from '../types/smartPlaylist';
import type { LocalAlbum } from '../types/local';
let localAlbums = new Map<string, LocalAlbum>();
export const sessionLocalAlbum = (id: string) => localAlbums.get(id);
const check = (current: () => boolean) => { if (!current()) throw 'SESSION_CANCELLED'; };
export async function checkSessionSource(source: SmartSource, current: () => boolean): Promise<void> {
  check(current);
  const available = await smartPlaylistsService.sources(); check(current);
  if (source.kind === 'local' ? !available.local : source.kind === 'plex' ? available.plex_server_id !== source.server_id
    : available.navidrome_account_id !== source.account_id) throw 'SESSION_SOURCE';
}
export async function sessionCatalog(source: SmartSource, current: () => boolean): Promise<SessionCandidate[]> {
  await checkSessionSource(source, current);
  if (source.kind !== 'local') return smartPlaylistsService.catalog(source, current);
  // getLocalAlbums already schedules its own source read. Wrapping it in the
  // same queue would make the outer job wait forever on its own inner job.
  const albums = await audioService.getLocalAlbums();
  check(current); await checkSessionSource(source, current);
  localAlbums = new Map(albums.map(album => [album.id, album]));
  return albums.map(album => ({ id: album.id, title: album.title, artist: album.artist,
    year: album.year ? parseInt(album.year, 10) || null : null, genres: album.genres ?? [],
    added_at: album.added_at ? Date.parse(album.added_at) / 1000 || null : null,
    scope_id: album.source_id ?? undefined, scope_label: album.source_label ?? undefined }));
}
export async function sessionAlbumTracks(source: SmartSource, album: SessionCandidate, current: () => boolean) {
  await checkSessionSource(source, current);
  let tracks;
  if (source.kind === 'local') {
    const groups = await localSourceReadQueue(() => { check(current); return audioService.getLocalAlbumTracks(album.id); });
    tracks = [...groups].sort((a, b) => a.disc.number - b.disc.number).flatMap(group => group.tracks)
      .filter(track => track.item_type === 'file').map(track => ({ title: track.title ?? track.name,
        artist: track.artist?.trim() || album.artist, album: track.album ?? album.title,
        media_locator: { kind: 'local' as const, uri: track.path }, duration: track.duration }));
  } else if (source.kind === 'plex') {
    const result = await plexService.getAlbumTracks(album.id); check(current);
    tracks = result.map(track => ({ title: track.title, artist: track.artist?.trim() || album.artist,
      album: track.album_title ?? album.title, media_locator: track.media_locator, plex_image: track.thumb ?? album.image,
      duration: track.duration_ms !== undefined ? track.duration_ms / 1000 : track.duration }));
  } else {
    const nav = await navidromeService.status(); check(current);
    if (!nav.connection_id || nav.account_id !== source.account_id) throw 'SESSION_SOURCE';
    const detail = await navidromeService.album(nav.connection_id, album.id); check(current);
    const confirmed = await navidromeService.status(); check(current);
    if (confirmed.connection_id !== nav.connection_id || confirmed.account_id !== source.account_id) throw 'SESSION_SOURCE';
    tracks = [...detail.songs].sort((a, b) => a.disc - b.disc || a.track - b.track).map(song => navidromeQueueTrack(nav, song));
  }
  check(current); await checkSessionSource(source, current);
  sessionAlbum(album, tracks, source, 'discovery'); // Refuse incomplete/foreign batches, never truncate albums.
  return tracks;
}
