import type { AppConfig } from "../types/config";

export function initialMediaSource(config: Pick<AppConfig, "plex_token"> | null): "plex" | "local" | null {
  if (!config) return null;
  return config.plex_token?.trim() ? "plex" : "local";
}
