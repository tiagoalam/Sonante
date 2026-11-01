import { sourceEnabled } from "./musicSources.ts";
import type { AppConfig } from "../types/config";

export function initialMediaSource(config: Pick<AppConfig, "plex_token" | "sources"> | null): "plex" | "local" | "navidrome" | "radio" | null {
  if (!config) return null;
  if (sourceEnabled(config, 'plex') && config.plex_token?.trim()) return 'plex';
  if (sourceEnabled(config, 'local')) return 'local';
  if (sourceEnabled(config, 'navidrome')) return 'navidrome';
  return 'radio';
}

export type MainSection = "navidrome" | "radio" | "history" | "search" | "local" | "plex" | "favorites" | "playlists" | "explore" | "equalizer";
export const MAIN_SECTION_STORAGE_KEY = "sonante.lastMainSection";
type SectionStorage = Pick<Storage, "getItem" | "setItem">;

function isMainSection(value: unknown): value is MainSection {
  return value === "navidrome" || value === "radio" || value === "history" || value === "search" || value === "local" || value === "plex" || value === "favorites" || value === "playlists" || value === "explore" || value === "equalizer";
}

function browserStorage(): SectionStorage | null {
  try { return globalThis.localStorage ?? null; } catch { return null; }
}

// Explore restores its principal library; legacy saved Explore values also map to Local.
// Only explicit navigation writes. Bootstrap, wizard and availability fallback
// publish a section without overwriting a saved preference.
export function createMainSectionPreference(
  publish: (section: MainSection | null) => void,
  storage: () => SectionStorage | null = browserStorage,
) {
  let navigationEnabled = false;
  let restoringPlex = false;
  let fallback: MainSection = "local";
  return {
    restore(config: Pick<AppConfig, "first_run" | "plex_token" | "sources"> | null) {
      navigationEnabled = Boolean(config && !config.first_run);
      let preferred: unknown = null;
      if (navigationEnabled) {
        try { preferred = storage()?.getItem(MAIN_SECTION_STORAGE_KEY); } catch { /* Storage is optional. */ }
      }
      const initial = initialMediaSource(config);
      fallback = sourceEnabled(config, "local") ? "local" : sourceEnabled(config, "navidrome") ? "navidrome" : "radio";
      const section = navigationEnabled && isMainSection(preferred)
        ? preferred === "explore" ? (sourceEnabled(config, 'local') ? 'local' : initial) :
          (preferred === 'local' || preferred === 'plex' || preferred === 'navidrome') && !sourceEnabled(config, preferred) ? initial :
          preferred === 'plex' && initial !== 'plex' ? initial : preferred
        : initial;
      restoringPlex = navigationEnabled && preferred === "plex" && section === "plex";
      publish(section);
    },
    finishSetup(config: Pick<AppConfig, "first_run" | "plex_token" | "sources">) {
      navigationEnabled = !config.first_run;
      restoringPlex = false;
      publish(initialMediaSource(config));
    },
    navigate(section: MainSection) {
      if (!navigationEnabled || !isMainSection(section)) return;
      restoringPlex = false;
      publish(section);
      try { storage()?.setItem(MAIN_SECTION_STORAGE_KEY, section === "explore" ? "local" : section); } catch { /* Navigation still works. */ }
    },
    plexDiscoveryFinished(available: boolean) {
      if (!restoringPlex) return;
      restoringPlex = false;
      if (!available) publish(fallback);
    },
  };
}
