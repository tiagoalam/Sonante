import type { PersonalCollectionTarget, PersonalCollectionArtwork } from "../services/sqliteLibrary";
import type { PlexImageRef } from "../types/plex";
export type CollectionCover = { kind: "local"; src: string } | { kind: "plex"; image: PlexImageRef; scope: string } | { kind: "subsonic"; connection: string; id: string };
export function resolvePersonalCollectionArtwork(target: PersonalCollectionTarget | null, services: {
  local: (path: string, current: () => boolean) => Promise<string | null>;
  artwork: PersonalCollectionArtwork | null;
}, current?: () => boolean): Promise<CollectionCover | null>;
