import { deriveAlbumGenres, filterAlbumsByGenre, genreComparisonKey } from "./albumGenres.ts";

interface GenreAlbum { genres?: readonly string[]; }
const placeholders = new Set(["unknown", "desconhecido", "desconhecida", "unknown genre", "gênero desconhecido", "genero desconhecido", "not provided", "não informado", "na", "n/a", "none", "sem gênero", "sem genero"]);
// Recognition controls ambiguous comma/slash parsing; it never aliases or rewrites genres.
const recognizedListNames = new Set(["big beat", "hip hop", "hip-hop", "breakbeat hardcore", "acid breaks", "industrial",
  "rock", "pop", "jazz", "blues", "classical", "electronic", "soul", "funk", "r&b", "reggae", "folk", "country",
  "metal", "ambient", "house", "techno", "drum and bass", "drum & bass",
  "chicago blues", "unplugged", "rock & roll", "rock and roll", "rock-and-roll", "rock \'n\' roll"]);
const slashExceptions = new Set(["pop/rock", "r&b/soul"]);
const rockDiscoveryFamily = new Set(["rock", "rock & roll", "rock and roll"]);

// Explicit Explore presentation family only; other styles and original tags stay intact.
export function exploreGenreDisplayName(value: string): string {
  return rockDiscoveryFamily.has(genreComparisonKey(value)) ? "Rock" : value;
}

export function isExploreGenreName(value: string): boolean {
  const key = genreComparisonKey(value);
  return !!key && !placeholders.has(key);
}

/** Discovery view only. Semicolon/pipe separate simple unquoted lists.
 * Comma/slash lists require every component in the bounded recognition set, except
 * two preserved composite labels. Quotes/brackets around a delimited value,
 * empty components, colon/plus/backslash, ellipses, >16 components or >96-character atoms are
 * ambiguous and omitted. Whitespace, hyphens, ampersands and 'and' are never
 * separators. NFC/case/whitespace comparison reuses the existing genre key.
 * After parsing, only Rock / Rock & Roll / Rock and Roll project to Rock.
 * No catalog, tags, universal style identity or source identity is changed.
 */
export function exploreGenreNames(album: GenreAlbum): string[] {
  const result = new Map<string, string>();
  for (const raw of Array.isArray(album.genres) ? album.genres : []) {
    if (typeof raw !== "string" || raw.length > 4096 || /[\u0000-\u001f\u007f:+\\]|…|\.\.\./u.test(raw)) continue;
    if (/[,;|/]/u.test(raw) && (/["()[\]{}]/u.test(raw) || /(?:^|[,;|/])\s*'|'\s*(?:$|[,;|/])/u.test(raw))) continue;
    const parts = raw.split(/[,;|]/u).map(part => part.normalize("NFC").trim().replace(/\s+/gu, " "));
    if (parts.length > 16 || parts.some(part => !part)) continue;
    const parsed: string[] = []; let ambiguous = false;
    for (const part of parts) {
      if (!isExploreGenreName(part)) continue;
      if (!part.includes("/")) {
        if (part.length > 96) { ambiguous = true; break; }
        parsed.push(part); continue;
      }
      const slash = part.split("/").map(value => value.trim());
      const composite = slash.join("/");
      if (slashExceptions.has(genreComparisonKey(composite))) { parsed.push(composite); continue; }
      if (slash.length > 16 || slash.some(value => !recognizedListNames.has(genreComparisonKey(value)))) { ambiguous = true; break; }
      parsed.push(...slash);
    }
    if (ambiguous || parsed.length > 16) continue;
    if (raw.includes(",") && parsed.some(name => !recognizedListNames.has(genreComparisonKey(name)) && !slashExceptions.has(genreComparisonKey(name)))) continue;
    for (const name of parsed) {
      const label = exploreGenreDisplayName(name), key = genreComparisonKey(label);
      if (!result.has(key)) result.set(key, label);
    }
  }
  return [...result.values()];
}

export function deriveExploreGenres(albums: readonly GenreAlbum[]) {
  return deriveAlbumGenres(albums.map(album => ({ genres: exploreGenreNames(album) })));
}

export function filterExploreAlbumsByGenre<T extends GenreAlbum>(albums: readonly T[], selected: string): T[] {
  const key = genreComparisonKey(exploreGenreDisplayName(selected));
  return albums.filter(album => filterAlbumsByGenre([{ genres: exploreGenreNames(album) }], key).length > 0);
}
