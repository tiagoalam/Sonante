import type { LocalAlbum, LocalAlbumTagPresence } from "../types/local";

export const mainMetadataFields = ["album", "artist", "album_artist", "date"] as const;
export const metadataPresenceFields = [...mainMetadataFields, "genre"] as const;
export type MetadataPresenceField = typeof metadataPresenceFields[number];
export type MetadataPresenceCategory = "all" | "filled" | "missing" | "unevaluated";

// Reject absent/partial/malformed evidence; displayed fallbacks and genre unions are never evidence.
export function validCatalogTagPresence(album: LocalAlbum): LocalAlbumTagPresence | null {
  const evidence = album.tag_presence;
  if (!evidence || typeof evidence !== "object" || !Number.isSafeInteger(album.track_count) || album.track_count <= 0
    || evidence.evaluated_track_count !== album.track_count || !evidence.missing || typeof evidence.missing !== "object"
    || metadataPresenceFields.some(field => !Number.isSafeInteger(evidence.missing[field])
      || evidence.missing[field] < 0 || evidence.missing[field] > evidence.evaluated_track_count)) return null;
  return evidence;
}

export function catalogPresenceCategory(album: LocalAlbum): Exclude<MetadataPresenceCategory, "all"> {
  const evidence = validCatalogTagPresence(album);
  return !evidence ? "unevaluated" : mainMetadataFields.some(field => evidence.missing[field] > 0) ? "missing" : "filled";
}

export function catalogPresenceCounts(albums: readonly LocalAlbum[]) {
  const categories = { all: albums.length, filled: 0, missing: 0, unevaluated: 0 };
  const fields = { album: 0, artist: 0, album_artist: 0, date: 0, genre: 0 };
  for (const album of albums) {
    categories[catalogPresenceCategory(album)]++;
    const evidence = validCatalogTagPresence(album);
    if (evidence) for (const field of metadataPresenceFields) if (evidence.missing[field] > 0) fields[field]++;
  }
  return { categories, fields, evaluated: albums.length - categories.unevaluated };
}

export function filterCatalogPresence(albums: readonly LocalAlbum[], query: string, category: MetadataPresenceCategory, fields: readonly MetadataPresenceField[]): LocalAlbum[] {
  const search = query.trim().toLocaleLowerCase();
  return albums.filter(album => (category === "all" || catalogPresenceCategory(album) === category)
    && `${album.title}\n${album.artist}\n${album.year ?? ""}`.toLocaleLowerCase().includes(search)
    && fields.every(field => (validCatalogTagPresence(album)?.missing[field] ?? 0) > 0));
}
