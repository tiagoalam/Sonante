interface AlbumGenres { genres?: readonly string[]; }

export const genreComparisonKey = (value: string): string =>
  value.normalize("NFC").trim().replace(/\s+/gu, " ").toLowerCase();

function names(album: AlbumGenres): string[] {
  return Array.isArray(album.genres) ? album.genres.filter((value): value is string =>
    typeof value === "string" && value.length <= 4096 && !!value.trim()
    && !/[\u0000-\u001f\u007f]/u.test(value)) : [];
}

export function albumGenreOptions(albums: readonly AlbumGenres[]): { value: string; label: string }[] {
  const options = new Map<string, string>();
  for (const album of albums) for (const name of names(album)) {
    const key = genreComparisonKey(name);
    if (!options.has(key)) options.set(key, name);
  }
  return [...options].sort(([left], [right]) => left.localeCompare(right))
    .map(([value, label]) => ({ value, label }));
}

export function deriveAlbumGenres(albums: readonly AlbumGenres[]): { value: string; label: string; albumCount: number }[] {
  const counts = new Map<string, number>();
  for (const album of albums) {
    for (const key of new Set(names(album).map(genreComparisonKey))) {
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
  }
  return albumGenreOptions(albums).map(option => ({ ...option, albumCount: counts.get(option.value) ?? 0 }));
}

export function validAlbumGenre(selected: string | null, options: readonly { value: string }[]): string | null {
  return selected !== null && options.some(option => option.value === selected) ? selected : null;
}

export function filterAlbumsByGenre<T extends AlbumGenres>(albums: readonly T[], selected: string | null): T[] {
  return albums.filter(album => selected === null || names(album).some(name => genreComparisonKey(name) === selected));
}
