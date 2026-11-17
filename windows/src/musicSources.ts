// Keep the original source cards/settings; expose only implemented platforms.
export * from "../../src/utils/musicSources";
export const MUSIC_SOURCES=["local","plex","navidrome"] as const;
