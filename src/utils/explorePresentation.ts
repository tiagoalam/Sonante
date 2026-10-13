import type { ExploreEntry } from "./explore.ts";

const normalize = (value: string) => value.normalize("NFC").trim().replace(/\s+/gu, " ").toLowerCase().normalize("NFC");
const generic = new Set([
  "unknown", "desconhecido", "desconhecida", "unknown artist", "artista desconhecido",
  "unknown album", "álbum desconhecido", "album desconhecido", "unknown title",
  "various artists", "vários artistas", "varios artistas", "va", "v.a.", "v/a",
  "untitled", "sem título", "sem titulo", "sem artista", "not provided", "não informado",
  "artist not provided", "artista não informado", "title not provided", "título não informado",
  "n/a", "na", "none",
]);

function usableText(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const text = normalize(value);
  return !text || generic.has(text) || !/[\p{L}\p{N}]/u.test(text) || /[\u0000-\u001f\u007f]/u.test(text) ? null : text;
}

// Artist diversity uses the same surface normalization; placeholders cannot
// establish that two unrelated entries belong to the same artist.
export function explorePresentationArtistKey(entry: ExploreEntry): string {
  const artist = usableText(entry.album.artist);
  return artist ? JSON.stringify(["artist", artist]) : JSON.stringify(["item", entry.key]);
}

function knownYear(value: unknown): number | null {
  if (typeof value === "number") return Number.isInteger(value) && value >= 1000 && value <= 9999 ? value : null;
  if (typeof value !== "string") return null;
  const match = value.trim().match(/^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/);
  if (!match) return null;
  const year = Number(match[1]); if (year < 1000) return null;
  if (match[2]) {
    const month = Number(match[2]), day = Number(match[3] ?? 1), date = new Date(Date.UTC(year, month - 1, day));
    if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
  }
  return year;
}

export interface ExplorePresentationGroup {
  key: string;
  candidate: ExploreEntry;
  members: readonly ExploreEntry[];
}

/** Suggestion diversity only, never album/file identity or edition equivalence.
 * artist is the existing album-artist display value (Local may fall back to a
 * track artist; Plex uses parentTitle). Only NFC, case and whitespace normalize;
 * punctuation, accents and all edition/live/disc qualifiers remain significant.
 * Unknown/generic text gets an individual item key. For each artist/title base,
 * missing years join a known year only when exactly one is present. If several
 * known years conflict, missing-year copies stay in their own bucket: they never
 * bridge those editions transitively. Same text/year can still hide distinct
 * unlabelled editions for this round; no metadata, catalog or ID is changed.
 * Groups and representatives are deterministic before any random sampling.
 */
export function groupExplorePresentation(pool: readonly ExploreEntry[], retained: readonly ExploreEntry[] = []): ExplorePresentationGroup[] {
  const descriptors = pool.map(entry => {
    const artist = usableText(entry.album.artist), title = usableText(entry.album.title);
    return { entry, base: artist && title ? JSON.stringify([artist, title]) : null, year: knownYear(entry.album.year) };
  });
  const years = new Map<string, Set<number>>();
  for (const { base, year } of descriptors) if (base && year !== null) {
    if (!years.has(base)) years.set(base, new Set()); years.get(base)!.add(year);
  }
  const groups = new Map<string, ExploreEntry[]>();
  for (const { entry, base, year } of descriptors) {
    const known = base ? years.get(base) : undefined;
    const bucket = year ?? (known?.size === 1 ? [...known][0] : null);
    const key = base ? JSON.stringify(["presentation", base, bucket]) : JSON.stringify(["item", entry.key]);
    if (!groups.has(key)) groups.set(key, []); groups.get(key)!.push(entry);
  }
  return [...groups].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([key, members]) => {
    const local = members.filter(entry => entry.source === "local"), preferred = local.length ? local : members;
    const kept = retained.flatMap(old => preferred.filter(entry => entry.key === old.key))[0];
    const candidate = kept ?? preferred.reduce((a, b) => b.key < a.key ? b : a);
    return { key, candidate, members };
  });
}
