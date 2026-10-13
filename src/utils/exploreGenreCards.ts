import { genreComparisonKey } from "./albumGenres.ts";
import { exploreGenreDisplayName } from "./exploreGenres.ts";

export const EXPLORE_GENRE_PALETTE = ["#632C45", "#234F75", "#285B48", "#765423", "#534176", "#255D65"] as const;
export type ExploreGenreIcon = "guitar" | "piano" | "mic" | "radio" | "headphones" | "music" | "disc" | "audio" | "playlist";
const icons: Readonly<Record<string, ExploreGenreIcon>> = {
  rock: "guitar", blues: "guitar", metal: "guitar", jazz: "piano", "hard bop": "piano",
  classical: "piano", "clássica": "piano", classica: "piano",
  "hip hop": "mic", "hip-hop": "mic", rap: "mic", pop: "radio",
  electronic: "headphones", "eletrônica": "headphones", eletronica: "headphones",
};

// Identity alone selects a finite palette entry. Source/order never participate.
// Exact icon hints and the explicit Rock presentation family only; no style inference.
export function exploreGenreCardStyle(value: string) {
  const key = genreComparisonKey(exploreGenreDisplayName(value));
  let hash = 2166136261;
  for (let i = 0; i < key.length; i++) hash = Math.imul(hash ^ key.charCodeAt(i), 16777619);
  return { background: EXPLORE_GENRE_PALETTE[(hash >>> 0) % EXPLORE_GENRE_PALETTE.length], icon: Object.prototype.hasOwnProperty.call(icons, key) ? icons[key] : "music" };
}

// Reserve preferred colors first, then resolve collisions by sorted identity.
// Four navigation cards fit the six-color palette. No arrival/order/history input.
export function assignExploreGenreColors(values: readonly string[]): Record<string, string> {
  const keys = [...new Set(values.map(genreComparisonKey))].sort();
  const assigned: Record<string, string> = Object.create(null), used = new Set<string>(), collisions: string[] = [];
  for (const key of keys) {
    const preferred = exploreGenreCardStyle(key).background;
    if (used.has(preferred)) collisions.push(key);
    else { assigned[key] = preferred; used.add(preferred); }
  }
  for (const key of collisions) {
    const preferred = exploreGenreCardStyle(key).background, start = EXPLORE_GENRE_PALETTE.indexOf(preferred);
    const color = EXPLORE_GENRE_PALETTE.map((_, i) => EXPLORE_GENRE_PALETTE[(start + i + 1) % EXPLORE_GENRE_PALETTE.length])
      .find(color => !used.has(color)) ?? preferred;
    assigned[key] = color; used.add(color);
  }
  return assigned;
}

// Reserve musical preferences first. Collisions use neutral music/media symbols,
// never a new genre classification. Four cards always fit these four alternatives.
export function assignExploreGenreIcons(values: readonly string[]): Record<string, ExploreGenreIcon> {
  const keys = [...new Set(values.map(genreComparisonKey))].sort();
  const assigned: Record<string, ExploreGenreIcon> = Object.create(null), used = new Set<ExploreGenreIcon>(), collisions: string[] = [];
  for (const key of keys) {
    const preferred = exploreGenreCardStyle(key).icon;
    if (used.has(preferred)) collisions.push(key);
    else { assigned[key] = preferred; used.add(preferred); }
  }
  const alternatives: readonly ExploreGenreIcon[] = ["music", "disc", "audio", "playlist"];
  for (const key of collisions) {
    const icon = alternatives.find(icon => !used.has(icon)) ?? exploreGenreCardStyle(key).icon;
    assigned[key] = icon; used.add(icon);
  }
  return assigned;
}
