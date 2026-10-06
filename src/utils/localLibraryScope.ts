import type { LocalAlbum } from "../types/local";

export interface LocalLibrarySourceOption {
  id: string;
  label: string;
}

export function localLibrarySourceOptions(
  albums: readonly LocalAlbum[],
): LocalLibrarySourceOption[] {
  const sources = new Map<string, LocalLibrarySourceOption>();
  for (const album of albums) {
    const id = album.source_id;
    if (id && !sources.has(id)) sources.set(id, { id, label: id });
  }
  return [...sources.values()].sort((left, right) => {
    const leftFolded = left.label.toLowerCase();
    const rightFolded = right.label.toLowerCase();
    if (leftFolded < rightFolded) return -1;
    if (leftFolded > rightFolded) return 1;
    if (left.label < right.label) return -1;
    if (left.label > right.label) return 1;
    return 0;
  });
}

export function validLocalLibrarySourceSelection(
  selectedSourceId: string | null,
  options: readonly LocalLibrarySourceOption[],
): string | null {
  if (selectedSourceId === null) return null;
  return options.some((source) => source.id === selectedSourceId)
    ? selectedSourceId
    : null;
}

export function filterLocalAlbumsByScope(
  albums: readonly LocalAlbum[],
  selectedSourceId: string | null,
  searchQuery: string,
): LocalAlbum[] {
  const query = searchQuery.toLowerCase();
  return albums.filter((album) => (
    (selectedSourceId === null || album.source_id === selectedSourceId)
    && (
      album.title.toLowerCase().includes(query)
      || album.artist.toLowerCase().includes(query)
    )
  ));
}

export function localLibraryScopeResetKey(
  selectedSourceId: string | null,
  searchQuery: string,
): string {
  return JSON.stringify([selectedSourceId, searchQuery]);
}
