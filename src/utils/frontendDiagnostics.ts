// Terminal diagnostics only. UI errors and IPC contracts remain unchanged.
const operations = [
  "app.bootstrap", "app.now-playing-listener", "app.window-title",
  "artist.load", "artist.play", "artwork.album-load",
  "artwork.folder-load", "artwork.local-enrich", "audio.health",
  "favorites.load", "favorites.load-tracks", "favorites.quick-play",
  "favorites.toggle", "local.album-location", "local.album-play",
  "local.directory-list", "local.favorite-toggle", "local.reconciliation-listener",
  "now-playing.analyzer-start", "now-playing.analyzer-stop", "now-playing.close",
  "now-playing.control", "now-playing.fullscreen-status", "now-playing.fullscreen-toggle",
  "now-playing.open", "player.next", "player.previous",
  "player.seek", "player.toggle", "player.volume",
  "playlists.add-tracks", "playlists.create", "playlists.create-with-tracks",
  "playlists.delete", "playlists.item-unavailable", "playlists.list",
  "playlists.load", "playlists.play", "playlists.remove-item",
  "playlists.rename", "playlists.reorder", "playlists.resolve-items",
  "plex.album-favorite", "plex.album-tracks", "plex.albums-load",
  "plex.collection-albums", "plex.collection-artwork", "plex.collections-load",
  "plex.favorite-toggle", "plex.libraries-load", "plex.pin-create",
  "plex.pin-status", "plex.quick-play", "plex.search",
  "plex.servers-discover", "queue.clear", "queue.load",
  "queue.play-index", "settings.load", "settings.local-folders-save",
  "settings.pick-folder", "settings.reload", "settings.reload-confirmed",
  "settings.save", "settings.save-status", "wizard.pick-folder"
] as const;
export type DiagnosticOperation = typeof operations[number];
const safeCodes = new Set([
  "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED",
  "LOCAL_SOURCE_REMOVAL_PREVIOUS_GENERATION_UNCONFIRMED",
  "LOCAL_SOURCE_REMOVAL_QUEUE_COMPLETION_UNCONFIRMED",
  "MANAGED_FOLDERS_SAVE_REMOVAL_UNCONFIRMED",
  "playlist_name_duplicate", "playlist_no_playable_items", "playlist_selected_item_unavailable",
  "EACCES", "ENOENT", "ETIMEDOUT", "ECONNRESET", "ECONNREFUSED", "EPIPE", "EBUSY",
  "ALSA_AVAILABILITY_UNCONFIRMED", "ALSA_PREFLIGHT_FAILED", "ALSA_PREFLIGHT_OUTPUT_BUSY",
  "ALSA_PREFLIGHT_OUTPUT_UNAVAILABLE", "CONFIG_PERSISTENCE_UNCERTAIN", "CONFIG_SAVE_BUSY",
  "LOCAL_FOLDERS_MIXED_CONFIG_CHANGE", "LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN", "LOCAL_SOURCE_FULL_RESCAN_REQUIRED",
  "LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH", "LOCAL_SOURCE_REMOVAL_DUPLICATE_URI", "LOCAL_SOURCE_REMOVAL_INVALID_CATALOG",
  "LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_INVALID", "LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_RESERVED", "LOCAL_SOURCE_REMOVAL_PLAN_CHANGED",
  "LOCAL_SOURCE_REMOVAL_PREVIOUS_CATALOG_MISMATCH", "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED", "LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED",
  "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED", "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE", "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE",
  "LOCAL_SOURCE_REMOVAL_QUEUE_INVALID", "LOCAL_SOURCE_REMOVAL_QUEUE_OWNER_INVALID", "LOCAL_SOURCE_REMOVAL_QUEUE_QUERY_FAILED",
  "LOCAL_SOURCE_REMOVAL_QUEUE_RECOVERY_UNCONFIRMED", "LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED", "LOCAL_SOURCE_STAGING_CLEANUP_FAILED",
  "LOCAL_SOURCE_TARGETED_PLAN_CHANGED", "MANAGED_FOLDERS_SAVE_PENDING", "album_catalog_failure",
  "album_not_found", "cue_virtual", "duplicate_expected_uri",
  "durability_failed", "file_not_found", "invalid_local_path",
  "invalid_response", "invalid_snapshot", "invalid_tag_update",
  "invalid_uri", "membership_changed", "metadata_preservation_failed",
  "missing", "mpd_unavailable", "output_busy",
  "output_unavailable", "outside_configured_roots", "permission_denied",
  "playback_error", "read_failed", "refresh_task_failed",
  "revision_conflict", "transaction_failed", "unauthorized",
  "unavailable", "unsupported_format", "update_command_failed"
]);

export interface ErrorDiagnostic {
  error_type: string;
  code?: string;
  hint?: "timeout" | "unavailable" | "permission_denied";
  mpd_ack_code?: number;
  mpd_command_index?: number;
  os_code?: number;
  http_status?: number;
  committed?: boolean;
  inspection_failed?: boolean;
}

// Never invoke getters, toString, toJSON or constructor/name on unknown objects.
function ownData(value: object, key: string): unknown {
  const descriptor = Object.getOwnPropertyDescriptor(value, key);
  return descriptor && "value" in descriptor ? descriptor.value : undefined;
}

export function summarizeDiagnostic(error: unknown): Readonly<ErrorDiagnostic> {
  const result: ErrorDiagnostic = { error_type: error === null ? "null" : typeof error };
  try {
    let message: unknown = error;
    if (typeof error === "object" && error !== null) {
      const code = ownData(error, "code");
      if (typeof code === "string" && code.length <= 128 && safeCodes.has(code)) result.code = code;
      const status = ownData(error, "status");
      if (typeof status === "number" && Number.isInteger(status) && status >= 100 && status <= 599) {
        result.http_status = status;
      }
      const committed = ownData(error, "committed");
      if (typeof committed === "boolean") result.committed = committed;
      message = ownData(error, "message");
    }
    if (typeof message === "string") {
      const text = message.slice(0, 4096);
      const candidate = text.match(/^([A-Za-z][A-Za-z0-9_]*)(?=[:;.\s]|$)/)?.[1];
      if (!result.code && candidate && safeCodes.has(candidate)) result.code = candidate;
      const ack = text.match(/\bACK \[(\d{1,5})@(\d{1,5})\]/);
      if (ack) {
        result.mpd_ack_code = Number(ack[1]);
        result.mpd_command_index = Number(ack[2]);
      }
      const os = text.match(/\(os error (\d{1,5})\)/);
      if (os) result.os_code = Number(os[1]);
      const http = text.match(/\bHTTP ([1-5]\d{2})\b/);
      if (http && result.http_status === undefined) result.http_status = Number(http[1]);
      // Text hints are not a diagnosis of the cause or a change in error classification.
      if (/timeout|timed out|tempo limite/i.test(text)) result.hint = "timeout";
      else if (/permission denied|permissão negada/i.test(text)) result.hint = "permission_denied";
      else if (/unavailable|indisponível/i.test(text)) result.hint = "unavailable";
    }
  } catch {
    // Proxies may throw even during descriptor inspection. Do not log that exception either.
    result.inspection_failed = true;
  }
  return Object.freeze(result);
}

function report(level: "error" | "warn", operation: DiagnosticOperation, error: unknown): void {
  // The allowlist also protects against unexpected runtime callers bypassing TS types.
  const context = (operations as readonly string[]).includes(operation) ? operation : "unknown-operation";
  console[level](`[Sonante] operation=${context}`, summarizeDiagnostic(error));
}

export function logError(operation: DiagnosticOperation, error?: unknown): void {
  report("error", operation, error);
}

export function logWarning(operation: DiagnosticOperation, error?: unknown): void {
  report("warn", operation, error);
}
