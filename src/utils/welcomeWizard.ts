import type { AppConfig } from "../types/config";

// Discovery still owns route selection. This only prevents a token-only UI
// from claiming a complete connection or allowing an incomplete setup.
export function wizardPlexConnectionReady(config: AppConfig): boolean {
  if (!config.plex_token.trim() || !config.plex_server_id?.trim()) return false;
  try {
    const route = new URL(config.plex_url);
    return ["http:", "https:"].includes(route.protocol) && Boolean(route.hostname)
      && !route.username && !route.password && !route.search && !route.hash;
  } catch { return false; }
}

export type WizardSaveErrorKey = "settings.localFolderOverlap" | "wizard.configPersistenceUncertain"
  | "wizard.deviceBusy" | "wizard.deviceUnavailable" | "wizard.saveError"
  | "wizard.libraryPreparationError" | "wizard.preparationIncomplete";

export function wizardSaveErrorKey(error: unknown): WizardSaveErrorKey {
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  if (message === "LOCAL_SOURCE_NEW_OVERLAP") return "settings.localFolderOverlap";
  if (message.startsWith("CONFIG_PERSISTENCE_UNCERTAIN:")) return "wizard.configPersistenceUncertain";
  if (message.startsWith("FIRST_RUN_PREPARATION_INCOMPLETE:")) return "wizard.preparationIncomplete";
  if (message === "FIRST_RUN_DATABASE_UNCONFIRMED" || message.startsWith("FIRST_RUN_DATABASE_UNCONFIRMED.")) return "wizard.libraryPreparationError";
  if (message === "ALSA_PREFLIGHT_OUTPUT_BUSY") return "wizard.deviceBusy";
  if (message === "ALSA_PREFLIGHT_OUTPUT_UNAVAILABLE") return "wizard.deviceUnavailable";
  return "wizard.saveError";
}
