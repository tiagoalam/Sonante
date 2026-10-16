import type { PlexDiscoverHub } from "../types/plex.ts";
export const discoverLanguage = (language: string): "pt-BR" | "en" => language.toLowerCase().startsWith("pt") ? "pt-BR" : "en";
// Aggregate families are classified by the backend's complete query contract.
// The legacy music title fallback uses its exact documented identity/context.
// Unknown/server-localized/custom titles pass through.
export function discoverHubTitle(hub: PlexDiscoverHub, t: (key: string) => string): string {
  if (hub.presentation === "artist_suggestions") return t("discover.artistSuggestions");
  if (hub.recent_family && !hub.library_id) return t(hub.recent_family === "added" ? "discover.standard.recentAdded" : hub.recent_family === "played_artists" ? "discover.standard.recentPlayedArtists" : "discover.standard.recentPlayed");
  return (hub.source_id ?? hub.id) === "home.music.recent" && hub.context === "hub.home.music.recent" && hub.title === "Recently Added Music"
    ? t("discover.standard.recentMusic") : hub.title;
}
export const discoverItemKey = (item: { server_id: string; library_id: string; kind: string; rating_key: string }) => JSON.stringify([item.server_id, item.library_id, item.kind, item.rating_key]);
