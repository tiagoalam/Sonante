import type { SmartAlbum, SmartSource } from '../types/smartPlaylist';
import { classifySmartLocalAlbumFamily, smartLocalGenreOptions } from './localGenreFamilies';
import { smartOptions } from './smartPlaylists';
type Genres = { genres: string[] };
export function sessionUsesGenreFamilies(source: SmartSource | null): boolean {
  return source?.kind === 'local' || source?.kind === 'navidrome';
}
// A session-navigation grouping only. Source tags, catalogs, smart-playlist
// exact rules and saved album/track references are never rewritten.
export function sessionGenreValues(albums: readonly SmartAlbum[], source: SmartSource | null): string[] {
  if (sessionUsesGenreFamilies(source)) return smartLocalGenreOptions(albums);
  return smartOptions(albums, 'genres');
}
export function sessionMatchesGenre(album: Genres, source: SmartSource | null, genre: string): boolean {
  if (!genre) return true;
  return sessionUsesGenreFamilies(source) ? classifySmartLocalAlbumFamily(album) === genre : album.genres.includes(genre);
}
