import { genreComparisonKey } from "./albumGenres.ts";

// Local presentation only. Exact aliases take precedence over delimiter parsing:
// spaces and hyphens never split names (e.g. Art Rock, Hard Bop, Hip-Hop).
// For multiple known families, the order below is the fixed priority; tag order,
// repetition and track counts never vote. Specific families precede broad Rock/Pop.
// Unknown individual tags do not override known tags. A delimited tag is usable
// only when EVERY part is known; otherwise that tag contributes only to Others.
// These are navigation rules, not a claim about an album's definitive musical genre.
const FAMILY_RULES = [
  ["brazilian", ["música brasileira", "brazilian", "brazilian music", "mpb", "samba", "samba rock", "samba-rock", "pagode", "bossa nova", "bossa-nova", "tropicália", "tropicalia", "tropicalismo", "choro", "chorinho", "forró", "baião", "sertanejo", "axé", "frevo", "maracatu", "funk carioca"]],
  ["classical", ["classical", "clássica", "música clássica", "classical music", "baroque", "barroco", "opera", "ópera", "chamber music", "orchestral", "early music", "contemporary classical"]],
  ["metal", ["metal", "heavy metal", "thrash metal", "death metal", "black metal", "doom metal", "power metal", "progressive metal", "symphonic metal", "nu metal", "alternative metal", "folk metal", "metalcore", "deathcore", "grindcore"]],
  ["electronic", ["electronic", "electronica", "eletrônica", "electrónica", "electro", "edm", "idm", "house", "deep house", "tech house", "techno", "trance", "drum and bass", "drum & bass", "drum n bass", "dnb", "dubstep", "breakbeat", "breakbeat hardcore", "big beat", "trip hop", "trip-hop", "downtempo", "ambient", "ambient techno", "synth-pop", "synth pop", "synthpop", "electropop", "disco", "nu disco", "garage house"]],
  ["jazz", ["jazz", "hard bop", "bop", "bebop", "be-bop", "post-bop", "post bop", "cool jazz", "free jazz", "smooth jazz", "jazz fusion", "jazz-rock", "jazz rock", "jazz funk", "jazz-funk", "soul jazz", "vocal jazz", "latin jazz", "modal jazz", "swing", "big band", "dixieland", "nu jazz", "acid jazz"]],
  ["blues", ["blues", "chicago blues", "delta blues", "electric blues", "country blues", "texas blues", "piedmont blues", "blues rock", "blues-rock"]],
  ["reggae", ["reggae", "roots reggae", "dub", "ska", "rocksteady", "dancehall", "ragga", "reggae fusion"]],
  ["hipHop", ["hip hop", "hip-hop", "hiphop", "rap", "gangsta rap", "conscious hip hop", "alternative hip hop", "boom bap", "trap", "grime"]],
  ["funkSoul", ["funk", "soul", "funk / soul", "funk/soul", "r&b", "r & b", "rhythm and blues", "rhythm & blues", "rnb", "neo soul", "neo-soul", "motown", "p-funk", "psychedelic soul", "northern soul", "contemporary r&b"]],
  ["folkCountry", ["folk", "country", "folk / country", "folk/country", "world, & country", "folk, world, & country", "americana", "bluegrass", "country rock", "country-rock", "folk rock", "folk-rock", "indie folk", "singer-songwriter", "singer songwriter", "traditional folk", "celtic", "celtic folk"]],
  ["rock", ["rock", "art rock", "hard rock", "classic rock", "progressive rock", "prog rock", "alternative rock", "indie rock", "psychedelic rock", "garage rock", "post-rock", "post rock", "punk", "punk rock", "post-punk", "post punk", "new wave", "grunge", "shoegaze", "rock and roll", "rock & roll", "rock 'n' roll", "rockabilly", "pop rock", "pop-rock"]],
  ["pop", ["pop", "dance pop", "dance-pop", "indie pop", "dream pop", "art pop", "baroque pop", "britpop", "k-pop", "j-pop", "chanson", "easy listening"]],
  ["latin", ["latin", "latina", "latin music", "salsa", "merengue", "bachata", "tango", "cumbia", "reggaeton", "latin pop"]],
  ["world", ["world", "world music", "traditional", "afrobeat", "afrobeats", "african", "flamenco", "fado", "indian classical", "raï"]],
  ["soundtrack", ["soundtrack", "soundtracks", "score", "film score", "trilha sonora", "trilhas sonoras", "stage & screen", "musical"]],
] as const;

export type LocalGenreFamily = typeof FAMILY_RULES[number][0] | "other";
interface AlbumGenres { genres?: readonly string[]; }

// Accent/case/spacing normalization is comparison-only; metadata is never rewritten.
const termKey = (term: string) => genreComparisonKey(term).normalize("NFD").replace(/\p{M}/gu, "");
const aliases = new Map<string, LocalGenreFamily>();
for (const [family, terms] of FAMILY_RULES) for (const term of terms) aliases.set(termKey(term), family);
const priority: readonly LocalGenreFamily[] = [...FAMILY_RULES.map(([family]) => family), "other"];
const preferred = (families: ReadonlySet<LocalGenreFamily>): LocalGenreFamily =>
  priority.find(family => families.has(family)) ?? "other";

function classifyTag(value: string): LocalGenreFamily {
  if (!value.trim() || value.length > 4096 || /\p{Cc}/u.test(value)) return "other";
  const exact = aliases.get(termKey(value));
  if (exact) return exact;
  const parts = value.split(/[/,;]/u).map(part => termKey(part));
  if (parts.length < 2 || parts.some(part => !part)) return "other";
  const families = new Set<LocalGenreFamily>();
  for (const part of parts) {
    // Only a standalone conjunction before a known list item is allowed.
    const family = aliases.get(part) ?? (part.startsWith("& ") ? aliases.get(part.slice(2)) : undefined);
    if (!family) return "other";
    families.add(family);
  }
  return preferred(families);
}

export function classifyLocalAlbumFamily(album: AlbumGenres): LocalGenreFamily {
  const families = new Set<LocalGenreFamily>();
  if (Array.isArray(album.genres)) for (const genre of album.genres) {
    if (typeof genre === "string") families.add(classifyTag(genre));
  }
  return preferred(families);
}

export function deriveLocalGenreFamilies(albums: readonly AlbumGenres[], label: (family: LocalGenreFamily) => string): {
  value: LocalGenreFamily; label: string; albumCount: number;
}[] {
  const counts = new Map<LocalGenreFamily, number>();
  for (const album of albums) {
    const family = classifyLocalAlbumFamily(album);
    counts.set(family, (counts.get(family) ?? 0) + 1);
  }
  return [...counts].map(([value, albumCount]) => ({ value, label: label(value), albumCount }))
    .sort((left, right) => left.value === "other" ? 1 : right.value === "other" ? -1
      : left.label.localeCompare(right.label) || left.value.localeCompare(right.value));
}

export function filterLocalAlbumsByFamily<T extends AlbumGenres>(albums: readonly T[], family: string): T[] {
  return albums.filter(album => classifyLocalAlbumFamily(album) === family);
}
