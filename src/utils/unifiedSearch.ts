import type {NavidromeStatus,NavidromeAlbum,NavidromeArtist,NavidromeSearchSong,NavidromeSearchResults} from "../types/navidrome";
import {navidromeQueueTrack} from "./navidromeTracks.ts";
import type { LocalAlbum, LocalItem } from "../types/local";
import type { PlexAlbum, PlexArtistResult, PlexSearchResults, PlexTrack } from "../types/plex";
import type { TrackMetadata } from "../types/audio";
import { deriveLocalArtists, type LocalArtistSummary } from "./localArtistNavigation.ts";
import { normalizeLocalNavigationText } from "./localAlbumNavigation.ts";
import { plexTrackArtist, plexTrackDurationSeconds } from "./plexTrackMetadata.ts";

export type SearchSource = "all" | "local" | "plex" | "navidrome";
export type SearchKind = "all" | "album" | "artist" | "track";
export type SearchRow =
  | { source: "local"; kind: "album"; key: string; album: LocalAlbum }
  | { source: "local"; kind: "artist"; key: string; artist: LocalArtistSummary }
  | { source: "local"; kind: "track"; key: string; track: LocalItem }
  | { source: "plex"; kind: "album"; key: string; album: PlexAlbum }
  | { source: "plex"; kind: "artist"; key: string; artist: PlexArtistResult }
  | { source: "plex"; kind: "track"; key: string; track: PlexTrack }
  | { source: "navidrome"; kind: "album"; key:string; album:NavidromeAlbum; connection:NavidromeStatus }
  | { source: "navidrome"; kind: "artist"; key:string; artist:NavidromeArtist; connection:NavidromeStatus }
  | { source: "navidrome"; kind: "track"; key:string; track:NavidromeSearchSong; connection:NavidromeStatus };
export interface SearchPreview { rows: SearchRow[]; limited: boolean; tracksUnavailable?: boolean }
export const SEARCH_PREVIEW_LIMIT = 20;

export function searchQuery(value: string): string | null {
  const query = value.trim();
  return !/[\0\r\n]/.test(query) && [...query].length >= 2 && [...query].length <= 160 ? query : null;
}
export function rowText(row: SearchRow): { title: string; subtitle: string } {
  if (row.kind === "album") return { title: row.album.title, subtitle: row.album.artist };
  if (row.kind === "artist") return { title: row.artist.name, subtitle: "" };
  if(row.source === "navidrome")return {title:row.track.title,subtitle:[row.track.artist,row.track.album].filter(Boolean).join(" · ")};
  return row.source === "local"
    ? { title: row.track.title || row.track.name, subtitle: [row.track.artist, row.track.album].filter(Boolean).join(" · ") }
    : { title: row.track.title, subtitle: [plexTrackArtist(row.track), row.track.album_title].filter(Boolean).join(" · ") };
}
export function searchRank(title: string, query: string): number {
  const text = normalizeLocalNavigationText(title), normalized = normalizeLocalNavigationText(query);
  return text === normalized ? 0 : text.startsWith(normalized) ? 1 : text.includes(normalized) ? 2 : 3;
}
export function sortSearchRows(rows: readonly SearchRow[], query: string): SearchRow[] {
  return [...rows].sort((a, b) => searchRank(rowText(a).title, query) - searchRank(rowText(b).title, query)
    || rowText(a).title.localeCompare(rowText(b).title) || a.key.localeCompare(b.key));
}

// Normalize album/artist metadata once per confirmed catalog, never once per keystroke.
export function localSearchIndex(albums: readonly LocalAlbum[]) {
  const rows: SearchRow[] = [
    ...albums.map(album => ({ source: "local", kind: "album", key: `local:album:${album.id}`, album }) as const),
    ...deriveLocalArtists(albums).map(artist => ({ source: "local", kind: "artist", key: `local:artist:${artist.id}`, artist }) as const),
  ];
  const index = rows.map(row => ({ row, text: normalizeLocalNavigationText(Object.values(rowText(row)).join(" ")) }));
  return (query: string): SearchPreview => {
    if (!searchQuery(query)) return { rows: [], limited: false };
    const terms = normalizeLocalNavigationText(query).split(/\s+/).filter(Boolean);
    const matches = index.filter(entry => terms.every(term => entry.text.includes(term))).map(entry => entry.row);
    const sorted = sortSearchRows(matches, query), result: SearchRow[] = [];
    let limited = false;
    for (const kind of ["album", "artist"] as const) {
      const group = sorted.filter(row => row.kind === kind);
      result.push(...group.slice(0, SEARCH_PREVIEW_LIMIT)); limited ||= group.length > SEARCH_PREVIEW_LIMIT;
    }
    return { rows: result, limited };
  };
}

export function localTrackRows(items: readonly LocalItem[]): SearchRow[] {
  const seen = new Set<string>();
  return items.filter(item => item.item_type === "file" && item.path && !seen.has(item.path) && Boolean(seen.add(item.path)))
    .slice(0, SEARCH_PREVIEW_LIMIT).map(track => ({ source: "local", kind: "track", key: `local:track:${track.path}`, track }));
}
export function plexSearchPreview(results: PlexSearchResults, serverId: string): SearchPreview {
  const all: SearchRow[] = [];
  for (const album of results.albums) {
    if (album.server_id && album.server_id !== serverId || album.thumb && album.thumb.server_id !== serverId) throw Error("SEARCH_CONTEXT_CHANGED");
    if (album.rating_key) all.push({ source: "plex", kind: "album", key: `plex:album:${album.rating_key}`, album });
  }
  for (const artist of results.artists) {
    if (artist.thumb && artist.thumb.server_id !== serverId) throw Error("SEARCH_CONTEXT_CHANGED");
    if (artist.rating_key) all.push({ source: "plex", kind: "artist", key: `plex:artist:${artist.rating_key}`, artist });
  }
  for (const track of results.tracks) {
    if (track.media_locator.kind === "plex" && track.media_locator.server_id !== serverId || track.thumb && track.thumb.server_id !== serverId) throw Error("SEARCH_CONTEXT_CHANGED");
    if (track.rating_key) all.push({ source: "plex", kind: "track", key: `plex:track:${track.rating_key}`, track });
  }
  const seen = new Set<string>(), rows: SearchRow[] = [];
  let limited = results.albums.length >= 12 || results.artists.length >= 12 || results.tracks.length >= 12;
  for (const kind of ["album", "artist", "track"] as const) {
    const group = all.filter(row => row.kind === kind && !seen.has(row.key) && Boolean(seen.add(row.key)));
    rows.push(...group.slice(0, SEARCH_PREVIEW_LIMIT)); limited ||= group.length >= SEARCH_PREVIEW_LIMIT;
  }
  return { rows, limited };
}
export function searchTrackMetadata(row: Extract<SearchRow, { kind: "track" }>): TrackMetadata {
  if(row.source === "navidrome")return navidromeQueueTrack(row.connection,row.track);
  if (row.source === "local") return { title: row.track.title || row.track.name, artist: row.track.artist || "", album: row.track.album || "",
    media_locator: { kind: "local", uri: row.track.path }, duration: row.track.duration, thumb: null };
  return { title: row.track.title, artist: plexTrackArtist(row.track), album: row.track.album_title || "",
    media_locator: row.track.media_locator, duration: plexTrackDurationSeconds(row.track), plex_image: row.track.thumb ?? null };
}

export function navidromeSearchPreview(results:NavidromeSearchResults,connection:NavidromeStatus):SearchPreview{
 if(results.connection_id!==connection.connection_id||results.account_id!==connection.account_id)throw Error("SEARCH_CONTEXT_CHANGED");
 const all:SearchRow[]=[...results.albums.map(album=>({source:"navidrome",kind:"album",key:`navidrome:${results.account_id}:album:${album.id}`,album,connection}) as const),
  ...results.artists.map(artist=>({source:"navidrome",kind:"artist",key:`navidrome:${results.account_id}:artist:${artist.id}`,artist,connection}) as const),
  ...results.songs.map(track=>({source:"navidrome",kind:"track",key:`navidrome:${results.account_id}:track:${track.id}`,track,connection}) as const)];
 const seen=new Set<string>(),rows:SearchRow[]=[];let limited=false;
 for(const kind of ["album","artist","track"] as const){const group=all.filter(row=>row.kind===kind&&!seen.has(row.key)&&Boolean(seen.add(row.key)));rows.push(...group.slice(0,SEARCH_PREVIEW_LIMIT));limited ||= group.length>SEARCH_PREVIEW_LIMIT;}
 return {rows,limited};
}

export interface SearchLaneState<T> { phase: "idle" | "loading" | "ready" | "error"; value: T | null }
// Each source has one in-flight request and one replaceable pending request.
// Clearing a visit invalidates publication without spawning another concurrent worker.
export class SearchLane<T> {
  private state: SearchLaneState<T> = { phase: "idle", value: null };
  private listeners = new Set<() => void>();
  private generation = 0;
  private running = false;
  private key: string | null = null;
  private pending: { generation: number; load: (current: () => boolean) => Promise<T> } | null = null;
  private readonly failed: (error: unknown) => void;
  constructor(failed: (error: unknown) => void = () => {}) { this.failed = failed; }
  getSnapshot = (): SearchLaneState<T> => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(state: SearchLaneState<T>) { this.state = state; this.listeners.forEach(listener => listener()); }
  clear = () => { this.generation++; this.pending = null; this.key = null; this.publish({ phase: "idle", value: null }); };
  cancel = () => {
    this.generation++; this.pending = null;
    if (this.state.phase === "loading") { this.key = null; this.publish({ phase: "idle", value: null }); }
  };
  request(key: string, load: (current: () => boolean) => Promise<T>) {
    if (this.key === key && (this.state.phase === "ready" || this.state.phase === "loading")) return;
    this.key = key; this.pending = { generation: ++this.generation, load };
    this.publish({ phase: "loading", value: null }); this.drain();
  }
  private drain() {
    if (this.running || !this.pending) return;
    const job = this.pending; this.pending = null; this.running = true;
    const current = () => job.generation === this.generation;
    Promise.resolve().then(async () => {
      if (!current()) return;
      const value = await job.load(current);
      if (current()) this.publish({ phase: "ready", value });
    }).catch(error => {
      if (current()) { this.publish({ phase: "error", value: null }); this.failed(error); }
    }).finally(() => { this.running = false; this.drain(); });
  }
}
