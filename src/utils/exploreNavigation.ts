import { sourceEnabled } from "./musicSources.ts";
import type { AppConfig } from "../types/config";

export type ExploreSource = "local" | "plex" | "navidrome";

// Configuration determines the available tabs, never a transient catalog/network result.
export function configuredExploreSources(config: Pick<AppConfig, "local_folders" | "plex_token" | "sources">, navidromeConfigured = false): ExploreSource[] {
  const sources: ExploreSource[] = [];
  if (sourceEnabled(config, "local") && config.local_folders.length) sources.push("local");
  if (sourceEnabled(config, "plex") && config.plex_token.trim()) sources.push("plex");
  if (sourceEnabled(config, "navidrome") && navidromeConfigured) sources.push("navidrome");
  return sources;
}

export function exploreSourceFor(sources: readonly ExploreSource[], preferred: ExploreSource): ExploreSource {
  return sources.includes(preferred) ? preferred : sources[0] ?? "local";
}
