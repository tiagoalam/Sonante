import type { CurrentMedia } from "../types/audio";
import type { LocalAlbum, LocalAlbumDiscTracks } from "../types/local";
import type { PlayerPlexNavigation, PlexAlbum, SelectedArtist } from "../types/plex";
import type { MainSection } from "./appBootstrap";
import { localArtistIdentity } from "./localArtistNavigation.ts";

export type PlayerNavigationTarget =
  | { kind: "local"; view: "album" | "artist"; albumId: string;
      verified: { groups: LocalAlbumDiscTracks[]; catalog: readonly LocalAlbum[] } }
  | { kind: "plex"; view: "album" | "artist"; album: PlexAlbum; artist: SelectedArtist; returnToArtist?: boolean };

export interface PlayerNavigationSession {
  target: PlayerNavigationTarget;
  returnSection: MainSection;
  revision: number;
}

export function visitPlayerTarget(
  previous: PlayerNavigationSession | null, target: PlayerNavigationTarget, section: MainSection,
): PlayerNavigationSession {
  return { target, returnSection: previous?.returnSection ?? section, revision: (previous?.revision ?? 0) + 1 };
}

export function currentMediaKey(media: CurrentMedia | null): string {
  if (!media) return "none";
  return JSON.stringify(media.kind === "local"
    ? [media.kind, media.uri, media.queue_index]
    : [media.kind, media.server_id, media.part_key, media.queue_index]);
}

export function localAlbumForMedia(media: CurrentMedia | null, albums: readonly LocalAlbum[]): LocalAlbum | null {
  if (media?.kind !== "local") return null;
  const parts = media.uri.split("/");
  if (parts.length < 2 || parts.some(part => !part || part === "." || part === "..")
    || /[\0\r\n]/.test(media.uri) || media.uri.includes("://")) return null;
  const directory = parts.slice(0, -1).join("/");
  // Exact disc directories also cover MPD virtual CUE tracks. Never use title/artist matching,
  // ancestor prefixes, or a CUE physical-folder fallback that could select a suppressed group.
  const candidates = albums.filter(album => (album.discs.length
    ? album.discs.map(disc => disc.folder_path)
    : [album.folder_path]).includes(directory));
  return candidates.length === 1 ? candidates[0] : null;
}

export function createPlayerAlbumLookup(
  target: Extract<PlayerNavigationTarget, { kind: "local" }> | undefined,
  catalog: () => readonly LocalAlbum[],
  load: (id: string) => Promise<LocalAlbumDiscTracks[]>,
): (id: string) => Promise<LocalAlbumDiscTracks[]> {
  let verified = target;
  return id => {
    if (verified && verified.verified.catalog !== catalog()) verified = undefined;
    // Also reusable across StrictMode effect replay, while this exact catalogue stays confirmed.
    if (verified?.albumId === id) return Promise.resolve(verified.verified.groups);
    return load(id);
  };
}

export class PlayerNavigationRequests {
  private generation = 0;
  private context = "";

  setContext(context: string): void {
    if (context === this.context) return;
    this.context = context;
    this.invalidate();
  }

  invalidate(): void { this.generation += 1; }

  async open(
    media: CurrentMedia | null,
    view: "album" | "artist",
    albums: readonly LocalAlbum[],
    lookup: (media: Extract<CurrentMedia, { kind: "plex" }>) => Promise<PlayerPlexNavigation>,
    publish: (target: PlayerNavigationTarget) => void,
    failed: () => void,
    displayedArtist?: string,
    lookupLocal?: (albumId: string) => Promise<LocalAlbumDiscTracks[]>,
  ): Promise<void> {
    const generation = ++this.generation;
    try {
      let target: PlayerNavigationTarget;
      if (media?.kind === "local") {
        const album = localAlbumForMedia(media, albums);
        if (!album || (view === "artist" && (!album.artist.trim() || !album.source_id
          || (displayedArtist !== undefined && localArtistIdentity(displayedArtist) !== localArtistIdentity(album.artist))))) {
          throw new Error("unresolved");
        }
        if (!lookupLocal) throw new Error("unverified URI");
        const groups = await lookupLocal(album.id);
        if (!groups.some(group => group.tracks.some(track => track.item_type === "file" && track.path === media.uri))) {
          throw new Error("URI absent from album");
        }
        target = { kind: "local", view, albumId: album.id, verified: { groups, catalog: albums } };
      } else if (media?.kind === "plex") {
        const result = await lookup(media);
        if (currentMediaKey(result.media) !== currentMediaKey(media)
          || !result.navigation?.album?.rating_key || !result.navigation.artist_rating_key
          || result.navigation.album.artist_rating_key !== result.navigation.artist_rating_key
          || !result.navigation.artist_name) throw new Error("unresolved identity");
        target = {
          kind: "plex", view, album: result.navigation.album,
          artist: { rating_key: result.navigation.artist_rating_key, name: result.navigation.artist_name },
        };
      } else { throw new Error("unresolved"); }
      if (generation === this.generation) publish(target);
    } catch {
      if (generation === this.generation) failed();
    }
  }
}
