import type { ExploreEntry } from "./explore.ts";
import { deriveExploreGenres, filterExploreAlbumsByGenre } from "./exploreGenres.ts";
import { groupExplorePresentation } from "./explorePresentation.ts";

export type ExploreTheme = { id: "decade"; value: number; current: boolean }
  | { id: "genre"; value: string; label: string };
export type ExploreThemeShelf = ExploreTheme & { selected: ExploreEntry[] };
export { isExploreGenreName } from "./exploreGenres.ts";

// Use only explicit valid years; date-shaped Local metadata must be a real date.
export function exploreThemeYear(value: unknown, currentYear: number): number | null {
  let year: number;
  if (typeof value === "number") year = value;
  else if (typeof value === "string") {
    const match = value.trim().match(/^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/);
    if (!match) return null;
    year = Number(match[1]);
    if (match[2]) {
      const month = Number(match[2]), day = Number(match[3] ?? 1), date = new Date(Date.UTC(year, month - 1, day));
      if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
    }
  } else return null;
  return Number.isInteger(year) && year >= 1000 && year <= currentYear ? year : null;
}

export function themeCandidates(pool: readonly ExploreEntry[], theme: ExploreTheme, year: number): ExploreEntry[] {
  return theme.id === "decade" ? pool.filter(entry => {
    const value = exploreThemeYear(entry.album.year, year);
    return value !== null && Math.floor(value / 10) * 10 === theme.value;
  }) : pool.filter(entry => filterExploreAlbumsByGenre([entry.album], theme.value).length > 0);
}

export function eligibleExploreThemes(pool: readonly ExploreEntry[], currentYear: number): ExploreTheme[] {
  const decades = new Set(pool.flatMap(entry => {
    const year = exploreThemeYear(entry.album.year, currentYear);
    return year === null ? [] : [Math.floor(year / 10) * 10];
  }));
  const themes: ExploreTheme[] = [
    ...[...decades].sort((a, b) => a - b).map(value => ({ id: "decade" as const, value, current: value === Math.floor(currentYear / 10) * 10 })),
    ...deriveExploreGenres(pool.map(entry => entry.album))
      .map(genre => ({ id: "genre" as const, value: genre.value, label: genre.label.trim() })),
  ];
  return themes.filter(theme => groupExplorePresentation(themeCandidates(pool, theme, currentYear)).length >= 10);
}

export function shuffleExploreSections<T>(items: readonly T[], random: () => number): T[] {
  const result = [...items];
  for (let i = result.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [result[i], result[j]] = [result[j], result[i]]; }
  return result;
}
