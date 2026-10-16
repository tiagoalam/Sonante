import type { PlexMixTrack } from "../types/plex.ts";
import type { TrackMetadata } from "../types/audio.ts";
import type { NewPlaylistItem } from "../types/playlist.ts";

export const MIX_LIMIT = 100;
export const mixTrackKey = (track: PlexMixTrack) => JSON.stringify([track.server_id, track.library_id, track.rating_key]);
export function shuffleMix<T>(items: T[], random: () => number): T[] {
  const result = [...items];
  for (let i = result.length - 1; i > 0; i--) {
    const value = random(), fraction = Number.isFinite(value) ? Math.max(0, Math.min(value, 1 - Number.EPSILON)) : 0;
    const j = Math.floor(fraction * (i + 1)); [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}
// Round-robin in a once-shuffled library order: response order never allocates
// vacancies. Tracks are deduplicated by source identity, never by display name.
export function balancedMix(pools: PlexMixTrack[][]): PlexMixTrack[] {
  const result: PlexMixTrack[] = [], seen = new Set<string>();
  for (let index = 0; result.length < MIX_LIMIT && pools.some(pool => index < pool.length); index++) {
    for (const pool of pools) {
      const track = pool[index]; if (!track) continue;
      const key = mixTrackKey(track); if (seen.has(key)) continue;
      seen.add(key); result.push(track); if (result.length === MIX_LIMIT) break;
    }
  }
  return result;
}
export const mixPlaybackMetadata = (tracks: PlexMixTrack[]): TrackMetadata[] => tracks.map(track => ({
  title: track.title, artist: track.artist ?? "", album: track.album_title ?? "",
  plex_image: track.thumb ?? null, media_locator: track.media_locator,
  duration: track.duration_ms ? track.duration_ms / 1000 : undefined,
}));
// Same locator/metadata contract as ArtistView, captured independently of later
// selection changes. No authenticated stream URL, artwork or library scan.
export const mixPlaylistItems = (tracks: PlexMixTrack[]): NewPlaylistItem[] => tracks.map(track => ({
  media_locator: { ...track.media_locator },
  metadata: { title: track.title, artist: track.artist || "", album: track.album_title || "",
    duration: track.duration_ms ? track.duration_ms / 1000 : track.duration },
}));
