import type { AppConfig } from "../types/config";

export function initialMediaSource(config: Pick<AppConfig, "plex_token"> | null): "plex" | "local" | null {
  if (!config) return null;
  return config.plex_token?.trim() ? "plex" : "local";
}

export type MainSection = "local" | "plex" | "favorites" | "playlists" | "explore";
export const MAIN_SECTION_STORAGE_KEY = "sonante.lastMainSection";
type SectionStorage = Pick<Storage, "getItem" | "setItem">;

function isMainSection(value: unknown): value is MainSection {
  return value === "local" || value === "plex" || value === "favorites" || value === "playlists" || value === "explore";
}

function browserStorage(): SectionStorage | null {
  try { return globalThis.localStorage ?? null; } catch { return null; }
}

// Only explicit navigation writes. Bootstrap, wizard and availability fallback
// publish a section without overwriting a saved preference.
export function createMainSectionPreference(
  publish: (section: MainSection | null) => void,
  storage: () => SectionStorage | null = browserStorage,
) {
  let navigationEnabled = false;
  let restoringPlex = false;
  return {
    restore(config: Pick<AppConfig, "first_run" | "plex_token"> | null) {
      navigationEnabled = Boolean(config && !config.first_run);
      let preferred: unknown = null;
      if (navigationEnabled) {
        try { preferred = storage()?.getItem(MAIN_SECTION_STORAGE_KEY); } catch { /* Storage is optional. */ }
      }
      const initial = initialMediaSource(config);
      const section = navigationEnabled && isMainSection(preferred)
        ? preferred === "plex" && initial !== "plex" ? "local" : preferred
        : initial;
      restoringPlex = navigationEnabled && preferred === "plex" && section === "plex";
      publish(section);
    },
    finishSetup(config: Pick<AppConfig, "first_run" | "plex_token">) {
      navigationEnabled = !config.first_run;
      restoringPlex = false;
      publish(initialMediaSource(config));
    },
    navigate(section: MainSection) {
      if (!navigationEnabled || !isMainSection(section)) return;
      restoringPlex = false;
      publish(section);
      try { storage()?.setItem(MAIN_SECTION_STORAGE_KEY, section); } catch { /* Navigation still works. */ }
    },
    plexDiscoveryFinished(available: boolean) {
      if (!restoringPlex) return;
      restoringPlex = false;
      if (!available) publish("local");
    },
  };
}
