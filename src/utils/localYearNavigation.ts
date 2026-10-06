import type { LocalAlbum } from "../types/local";
import { parseLocalAlbumYear } from "./localAlbumNavigation.ts";

export interface LocalYearSummary {
  year: number;
  albumCount: number;
}

export interface LocalDecadeSummary {
  decade: number;
  albumCount: number;
  years: LocalYearSummary[];
}

export interface LocalYearNavigation {
  decades: LocalDecadeSummary[];
  unknownCount: number;
}

export function decadeForYear(year: number): number {
  return Math.floor(year / 10) * 10;
}

export function deriveLocalYearNavigation(
  albums: readonly LocalAlbum[],
  currentYear = new Date().getFullYear(),
): LocalYearNavigation {
  const decades = new Map<number, Map<number, number>>();
  let unknownCount = 0;
  for (const album of albums) {
    const year = parseLocalAlbumYear(album.year, currentYear);
    if (year === null) {
      unknownCount += 1;
      continue;
    }
    const decade = decadeForYear(year);
    const years = decades.get(decade) ?? new Map<number, number>();
    years.set(year, (years.get(year) ?? 0) + 1);
    decades.set(decade, years);
  }
  return {
    decades: [...decades.entries()]
      .map(([decade, years]) => ({
        decade,
        albumCount: [...years.values()].reduce((total, count) => total + count, 0),
        years: [...years.entries()]
          .map(([year, albumCount]) => ({ year, albumCount }))
          .sort((left, right) => left.year - right.year),
      }))
      .sort((left, right) => right.decade - left.decade),
    unknownCount,
  };
}

export function filterLocalAlbumsByYear(
  albums: readonly LocalAlbum[],
  year: number,
  currentYear = new Date().getFullYear(),
): LocalAlbum[] {
  return albums.filter((album) => parseLocalAlbumYear(album.year, currentYear) === year);
}

export function filterLocalAlbumsWithoutYear(
  albums: readonly LocalAlbum[],
  currentYear = new Date().getFullYear(),
): LocalAlbum[] {
  return albums.filter((album) => parseLocalAlbumYear(album.year, currentYear) === null);
}

export function validLocalYearSelection(
  navigation: LocalYearNavigation,
  decade: number | null,
  year: number | null,
  unknown: boolean,
): { decade: number | null; year: number | null; unknown: boolean } {
  if (unknown) {
    return navigation.unknownCount > 0
      ? { decade: null, year: null, unknown: true }
      : { decade: null, year: null, unknown: false };
  }
  if (decade === null) return { decade: null, year: null, unknown: false };
  const decadeSummary = navigation.decades.find((item) => item.decade === decade);
  if (!decadeSummary) return { decade: null, year: null, unknown: false };
  if (year !== null && !decadeSummary.years.some((item) => item.year === year)) {
    return { decade, year: null, unknown: false };
  }
  return { decade, year, unknown: false };
}
