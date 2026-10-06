import type { PlexAlbum } from "../types/plex";
import { parsePlexAlbumYear, sortPlexAlbums } from "./plexAlbumNavigation.ts";

export interface PlexYearSummary {
  year: number;
  albumCount: number;
}

export interface PlexDecadeSummary {
  decade: number;
  albumCount: number;
  years: PlexYearSummary[];
}

export interface PlexYearNavigation {
  decades: PlexDecadeSummary[];
  unknownCount: number;
}

export function plexDecadeForYear(year: number): number {
  return Math.floor(year / 10) * 10;
}

export function derivePlexYearNavigation(
  albums: readonly PlexAlbum[],
  currentYear = new Date().getFullYear(),
): PlexYearNavigation {
  const decades = new Map<number, Map<number, number>>();
  let unknownCount = 0;
  for (const album of albums) {
    const year = parsePlexAlbumYear(album.year, currentYear);
    if (year === null) {
      unknownCount += 1;
      continue;
    }
    const decade = plexDecadeForYear(year);
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

export function plexAlbumsForYear(
  albums: readonly PlexAlbum[],
  year: number,
  currentYear = new Date().getFullYear(),
): PlexAlbum[] {
  return sortPlexAlbums(
    albums.filter((album) => parsePlexAlbumYear(album.year, currentYear) === year),
    "album_asc",
    currentYear,
  );
}

export function plexAlbumsWithoutYear(
  albums: readonly PlexAlbum[],
  currentYear = new Date().getFullYear(),
): PlexAlbum[] {
  return sortPlexAlbums(
    albums.filter((album) => parsePlexAlbumYear(album.year, currentYear) === null),
    "album_asc",
    currentYear,
  );
}
