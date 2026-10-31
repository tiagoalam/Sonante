import type { AppConfig } from "../types/config";

export type ExploreSource = "local" | "plex";

// Configuration determines the available tabs, never a transient catalog/network result.
export function configuredExploreSources(config: Pick<AppConfig, "local_folders" | "plex_token">): ExploreSource[] {
  const sources: ExploreSource[] = [];
  if (config.local_folders.length) sources.push("local");
  if (config.plex_token.trim()) sources.push("plex");
  return sources;
}

export function exploreSourceFor(sources: readonly ExploreSource[], preferred: ExploreSource): ExploreSource {
  return sources.includes(preferred) ? preferred : sources[0] ?? "local";
}
