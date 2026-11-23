import { sourceEnabled } from "./musicSources.ts";
import type { AppConfig } from "../types/config";

export function initialMediaSource(config: Pick<AppConfig, "plex_token" | "sources"> | null): "plex" | "local" | "navidrome" | "radio" | null {
  if (!config) return null;
  if (sourceEnabled(config, 'plex') && config.plex_token?.trim()) return 'plex';
  if (sourceEnabled(config, 'local')) return 'local';
  if (sourceEnabled(config, 'navidrome')) return 'navidrome';
  return 'radio';
}

export function initialSqliteMediaSource(config: Pick<AppConfig, "plex_token" | "sources"> | null): "plex" | "local" | "navidrome" | "radio" | null {
  if (!config) return null;
  if (sourceEnabled(config, "local")) return "local";
  return initialMediaSource(config);
}

export type MainSection = "editions" | "sessions" | "navidrome" | "radio" | "history" | "search" | "local" | "plex" | "favorites" | "playlists" | "explore" | "equalizer" | "autoeq";
export function disabledAutoEqFallback(section: MainSection | null, betaEnabled: boolean): MainSection | null {
  return section === "autoeq" && !betaEnabled ? "equalizer" : null;
}
export const MAIN_SECTION_STORAGE_KEY = "sonante.lastMainSection";
// This key remains backward compatible inside the dev064 WebView profile. The
// Tauri application identifier isolates it from the stable player's storage.
export const SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY = MAIN_SECTION_STORAGE_KEY;
export const SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY = "sonante.0.6.4-sqlite.lastPlexLibrary";
export const SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY = "sonante.0.6.4-sqlite.lastLocalSource";
type SectionStorage = Pick<Storage, "getItem" | "setItem">;

function isMainSection(value: unknown): value is MainSection {
  return value === "editions" || value === "sessions" || value === "navidrome" || value === "radio" || value === "history" || value === "search" || value === "local" || value === "plex" || value === "favorites" || value === "playlists" || value === "explore" || value === "equalizer" || value === "autoeq";
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
  storageKey = MAIN_SECTION_STORAGE_KEY,
  initialSource: (config: Pick<AppConfig, "plex_token" | "sources"> | null) => MainSection | null = initialMediaSource,
) {
  let navigationEnabled = false;
  let restoringPlex = false;
  let fallback: MainSection = "local";
  return {
    restore(config: Pick<AppConfig, "first_run" | "plex_token" | "sources" | "usage_mode"> | null) {
      navigationEnabled = Boolean(config && !config.first_run);
      let preferred: unknown = null;
      if (navigationEnabled) {
        try { preferred = storage()?.getItem(storageKey); } catch { /* Storage is optional. */ }
      }
      const initial = initialSource(config);
      fallback = sourceEnabled(config, "local") ? "local" : sourceEnabled(config, "navidrome") ? "navidrome" : "radio";
      const preferredSourceUnavailable = preferred === "local" && !sourceEnabled(config, "local")
        || preferred === "plex" && (!sourceEnabled(config, "plex") || !config?.plex_token?.trim())
        || preferred === "navidrome" && !sourceEnabled(config, "navidrome");
      const section = navigationEnabled && isMainSection(preferred)
        ? config?.usage_mode === "simple" && (preferred === "equalizer" || preferred === "autoeq") ? initial : preferred === "editions" || preferred === "sessions" ? initial : preferred === "explore" ? (sourceEnabled(config, 'local') ? 'local' : initial) :
          preferredSourceUnavailable ? initial : preferred
        : initial;
      restoringPlex = navigationEnabled && preferred === "plex" && section === "plex";
      publish(section);
    },
    finishSetup(config: Pick<AppConfig, "first_run" | "plex_token" | "sources">) {
      navigationEnabled = !config.first_run;
      restoringPlex = false;
      publish(initialSource(config));
    },
    navigate(section: MainSection) {
      if (!navigationEnabled || !isMainSection(section)) return;
      restoringPlex = false;
      publish(section);
      try { if (section !== "sessions" && section !== "editions") storage()?.setItem(storageKey, section === "explore" ? "local" : section); } catch { /* Navigation still works. */ }
    },
    plexDiscoveryFinished(available: boolean) {
      if (!restoringPlex) return;
      restoringPlex = false;
      if (!available) publish(fallback);
    },
  };
}

type PlexLibraryPreferenceStorage = Pick<Storage, "getItem" | "setItem">;

export function preferredSqlitePlexLibrary<T extends { key: string }>(
  libraries: readonly T[],
  serverId: string | null | undefined,
  storage: () => PlexLibraryPreferenceStorage | null = browserStorage,
): T | null {
  if (!serverId || libraries.length === 0) return libraries[0] ?? null;
  try {
    const stored = storage()?.getItem(SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY);
    if (!stored) return libraries[0] ?? null;
    const preference: unknown = JSON.parse(stored);
    if (!preference || typeof preference !== "object") return libraries[0] ?? null;
    const saved = preference as { serverId?: unknown; libraryKey?: unknown };
    if (saved.serverId !== serverId || typeof saved.libraryKey !== "string") return libraries[0] ?? null;
    return libraries.find(library => library.key === saved.libraryKey) ?? libraries[0] ?? null;
  } catch {
    return libraries[0] ?? null;
  }
}

export function saveSqlitePlexLibraryPreference(
  serverId: string | null | undefined,
  libraryKey: string,
  storage: () => PlexLibraryPreferenceStorage | null = browserStorage,
): void {
  if (!serverId || !libraryKey) return;
  try {
    storage()?.setItem(SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY, JSON.stringify({ serverId, libraryKey }));
  } catch { /* Persisting this preference is optional; Plex remains usable. */ }
}

export function readSqliteLocalSourcePreference(
  storage: () => SectionStorage | null = browserStorage,
): string | null {
  try {
    const value = storage()?.getItem(SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY)?.trim();
    return value && value.length <= 256 ? value : null;
  } catch { return null; }
}

export function saveSqliteLocalSourcePreference(
  sourceId: string | null,
  storage: () => SectionStorage | null = browserStorage,
): void {
  try { storage()?.setItem(SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY, sourceId ?? ""); }
  catch { /* The current library selection remains usable for this session. */ }
}
