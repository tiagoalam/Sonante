import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export type ScrobblingProvider = "lastfm" | "listenbrainz";
export interface ScrobblingServiceStatus { connected: boolean; enabled: boolean; username: string | null; queued: number; last_sent: number | null; error: string | null }
export interface ScrobblingStatus { revision: number; ready: boolean; storage_error: boolean; lastfm_authorizing: boolean; lastfm: ScrobblingServiceStatus; listenbrainz: ScrobblingServiceStatus }
export type ScrobblingAction =
  | { kind: "lastfm_begin"; api_key: string; api_secret: string }
  | { kind: "lastfm_finish" } | { kind: "lastfm_cancel" }
  | { kind: "listenbrainz_connect"; token: string }
  | { kind: "set_enabled"; provider: ScrobblingProvider; enabled: boolean }
  | { kind: "disconnect"; provider: ScrobblingProvider };
export interface ScrobblingActionResult { status: ScrobblingStatus; authorization_url: string | null }
export const scrobblingService = {
  status: () => invoke<ScrobblingStatus>("get_scrobbling"),
  action: (action: ScrobblingAction) => invoke<ScrobblingActionResult>("set_scrobbling", { action }),
  subscribe: (changed: (status: ScrobblingStatus) => void) => listen<ScrobblingStatus>("scrobbling-changed", event => changed(event.payload)),
};
const codes = new Set(["auth", "not_authorized", "expired", "network", "temporary", "rate_limited", "rejected", "protocol", "invalid", "queue_full", "storage_unavailable", "unavailable"]);
export function scrobblingError(error: unknown): string {
  if (typeof error !== "string") return "unavailable";
  const value = error.replace(/^SCROBBLING_/, "").toLowerCase();
  return codes.has(value) ? value : "unavailable";
}
