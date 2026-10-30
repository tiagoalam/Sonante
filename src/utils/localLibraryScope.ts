import type { LocalAlbum } from "../types/local";

export interface LocalLibrarySourceOption {
  id: string;
  label: string;
}

// Presentation only: never derive a managed source ID from a configured physical path.
export function localLibrarySelectorPresentation(folders: readonly string[]): { mode: "empty" | "single" | "multiple"; label?: string } {
  if (folders.length === 0) return { mode: "empty" };
  if (folders.length > 1) return { mode: "multiple" };
  const path = folders[0].replace(/\/+$/u, "");
  return { mode: "single", label: path.slice(path.lastIndexOf("/") + 1) || folders[0] };
}

export function localLibrarySourceOptions(
  albums: readonly LocalAlbum[],
): LocalLibrarySourceOption[] {
  const sources = new Map<string, LocalLibrarySourceOption>();
  for (const album of albums) {
    const id = album.source_id;
    if (id && !sources.has(id)) sources.set(id, { id, label: album.source_label?.trim() || id });
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

// Replace only the root's display name; navigation and playback retain the URI.
export function localLibraryPathLabel(path: string, sources: readonly LocalLibrarySourceOption[]): string {
  const slash = path.indexOf("/");
  const id = slash < 0 ? path : path.slice(0, slash);
  const label = sources.find(source => source.id === id)?.label ?? id;
  return label + (slash < 0 ? "" : path.slice(slash));
}
