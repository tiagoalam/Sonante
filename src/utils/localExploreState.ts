import type { ConfigSavePhase } from "../types/config.ts";
import type { ExploreSourceState } from "./explore.ts";
import { localCatalogActivity, type LocalAlbumCatalogState } from "./localAlbumCatalog.ts";

// A save block alone is not proof that an import is still progressing.
export function localExplorePresentation(input: {
  configured: boolean; starting: boolean; available: boolean; updating: boolean;
  catalog: LocalAlbumCatalogState; savePhase: ConfigSavePhase | null; saveStatusUncertain: boolean;
}): { state: ExploreSourceState; activity: "indexing" | "organizing" | null } {
  const { configured, starting, available, updating, catalog, savePhase, saveStatusUncertain } = input;
  const confirmedSave = !saveStatusUncertain && savePhase !== null && ["preparing", "removing", "indexing", "validating"].includes(savePhase);
  const result = (state: ExploreSourceState, activity: "indexing" | "organizing" | null = null) => ({ state, activity });
  if (!configured) return result("not_configured");
  if (saveStatusUncertain || catalog.error || catalog.saveBlocked && !confirmedSave) return result("unavailable");
  if (starting) return result("loading");
  if (!available) return result("unavailable");
  const activity = localCatalogActivity(updating, catalog) ?? (confirmedSave ? "indexing" : null);
  if (activity) return result("loading", activity);
  if (!catalog.loaded) return result("loading");
  return result(catalog.albums.length ? "ready" : "empty");
}
