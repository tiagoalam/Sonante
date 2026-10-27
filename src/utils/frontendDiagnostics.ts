// DevTools diagnostics only. UI errors remain unchanged.
const operations = [
  "player.request", "plex.album-play",
  "libraryRecovery.cancel", "app.bootstrap", "app.now-playing-listener", "app.window-title",
  "artist.load", "artist.play", "artwork.album-load",
  "artwork.folder-load", "artwork.local-enrich", "audio.health",
  "favorites.load", "favorites.load-tracks", "favorites.quick-play",
  "favorites.toggle", "local.album-location", "local.album-play",
  "local.directory-list", "local.favorite-toggle", "local.reconciliation-listener",
  "now-playing.analyzer-start", "now-playing.analyzer-stop", "now-playing.close",
  "now-playing.control", "now-playing.fullscreen-status", "now-playing.fullscreen-toggle",
  "now-playing.open", "player.next", "player.previous", "player.options",
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
  "ALSA_PREFLIGHT_OUTPUT_UNAVAILABLE", "CONFIG_PERSISTENCE_UNCERTAIN", "CONFIG_SAVE_BUSY", "FIRST_RUN_CATALOG_INDEXING",
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
  "local_artwork_busy", "favorites_busy",
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

let errorWindow = 0, errorLines = 0;
let suppressedErrors: Record<string, number> = {};
function report(level: "error" | "warn", operation: DiagnosticOperation, error: unknown): void {
  // The allowlist also protects against unexpected runtime callers bypassing TS types.
  const context = (operations as readonly string[]).includes(operation) ? operation : "unknown-operation";
  if (devDiagnosticsEnabled()) {
    const now = Date.now();
    if (now - errorWindow >= 10_000) { errorWindow = now; errorLines = 0; }
    if (errorLines++ >= 16) { suppressedErrors[context] = (suppressedErrors[context] ?? 0) + 1; return; }
  }
  console[level](`[Sonante] operation=${context}`, summarizeDiagnostic(error));
}

export function logError(operation: DiagnosticOperation, error?: unknown): void {
  report("error", operation, error);
}

export function logWarning(operation: DiagnosticOperation, error?: unknown): void {
  report("warn", operation, error);
}

// This existing mix event is DEV-only, independent of Explore's extra opt-in.
// Accept only bounded scalar fields, never an error or a response payload.
export function logPlexMixDiagnostic(stage: string, category: string, reason: string, observed: number): void {
  if (!import.meta.env?.DEV) return;
  try {
    if (!["ipc", "frontend"].includes(stage)
      || !["PLEX_DISCOVER_STYLE_UNSUPPORTED", "PLEX_DISCOVER_AUTH", "PLEX_DISCOVER_CONTEXT", "PLEX_DISCOVER_INVALID", "PLEX_DISCOVER_UNAVAILABLE"].includes(category)
      || !["operation_rejected", "response_scope", "response_shape", "track_or_locator_scope"].includes(reason)) return;
    const count = typeof observed === "number" && Number.isFinite(observed) ? Math.max(0, Math.min(101, Math.trunc(observed))) : 0;
    console.info("[Sonante] plex.mix.frontend", JSON.stringify({ stage, category, reason, observed: count }));
  } catch { /* Diagnostics must not affect selection or interaction. */ }
}

// Opt-in development snapshots. Never evaluate the builder in a production build.
let exploreWindow = 0, exploreLines = 0, exploreOmitted = 0;
export function devDiagnosticsEnabled(): boolean {
  return Boolean(import.meta.env?.DEV && import.meta.env.VITE_SONANTE_EXPLORE_DIAGNOSTICS === "1");
}
export function logExploreDiagnostic(build: () => object): void {
  if (!devDiagnosticsEnabled()) return;
  const now = Date.now();
  if (now - exploreWindow >= 10_000) { exploreWindow = now; exploreLines = 0; }
  if (exploreLines++ >= 16) { exploreOmitted++; return; }
  try {
    console.info("[Sonante] explore.diagnostic", JSON.stringify(build()));
  } catch { /* Diagnostics must not affect selection or interaction. */ }
}

const exploreAliases = { root: new Map<string, string>(), library: new Map<string, string>() };
export function exploreDiagnosticAlias(kind: keyof typeof exploreAliases, value: string): string {
  const aliases = exploreAliases[kind];
  if (!aliases.has(value) && aliases.size >= 256) return `${kind}-omitted`;
  if (!aliases.has(value)) aliases.set(value, `${kind}-${aliases.size + 1}`);
  return aliases.get(value)!;
}

// Metadata is untrusted too: omit path/URL/credential-like values entirely.
export function exploreDiagnosticText(value: string): string {
  if (/[\\/\u0000-\u001f\u007f]|(?:token|password|credential|secret|authorization)\s*[:=]|[a-z][a-z0-9+.-]*:/iu.test(value)) return "[redacted]";
  return value.slice(0, 80);
}


const correlatedCommands = ["get_playback_status", "get_mpd_status_snapshot", "get_queue", "set_window_title",
  "list_local_directory", "get_local_album_tracks", "get_local_cover", "get_local_albums", "get_favorites", "toggle_favorite"] as const;
const actionTargets = ["local", "plex", "explore", "favorites", "playlists", "settings", "about"] as const;
export type InteractionAction = typeof actionTargets[number];
export interface InteractionState {
  section: string | null; health: string; bootstrap: boolean; settings: boolean; about: boolean;
  wizard: boolean; queueConfirmation: boolean; playlistPicker: boolean; saveBlocked: boolean;
  catalogLoading: boolean; catalogAlbums: number;
}

// No arguments, responses, DOM text, paths or raw errors enter the observer.
// Timeouts are deliberately absent: a pending IPC stays pending until it settles.
export function createInteractionObservations(now = Date.now) {
  let sequence = 0, actionId: number | null = null, omitted = 0;
  const pending = new Map<number, { ipc: number; action: number | null; command: string; started: number }>();
  const counts = new Map<string, { started: number; completed: number; failed: number; slow: number }>();
  let recent: object[] = [];
  const record = (value: object) => { if (recent.length === 16) recent.shift(); recent.push(value); };
  return {
    action(target: unknown, phase: "entered" | "returned") {
      if (!(actionTargets as readonly unknown[]).includes(target)) return;
      if (phase === "entered") actionId = ++sequence;
      record({ action: actionId, target, phase });
      return actionId;
    },
    input(kind: "pointer" | "keyboard", sidebarHit: boolean, dialogHit: boolean) {
      record({ event: "input", kind, sidebarHit, dialogHit });
    },
    state(value: InteractionState) {
      const safe = { section: (actionTargets as readonly unknown[]).includes(value.section) ? value.section : null,
        health: ["available", "unavailable", "starting", "stopping"].includes(value.health) ? value.health : "unknown",
        bootstrap: !!value.bootstrap, settings: !!value.settings, about: !!value.about, wizard: !!value.wizard,
        queueConfirmation: !!value.queueConfirmation, playlistPicker: !!value.playlistPicker,
        saveBlocked: !!value.saveBlocked, catalogLoading: !!value.catalogLoading,
        catalogAlbums: Number.isSafeInteger(value.catalogAlbums) && value.catalogAlbums >= 0 ? value.catalogAlbums : 0 };
      record({ event: "react.commit", action: actionId, ...safe });
    },
    begin(command: string): number | undefined {
      if (!(correlatedCommands as readonly string[]).includes(command)) return;
      if (pending.size >= 128) { omitted++; return; }
      const ipc = ++sequence;
      pending.set(ipc, { ipc, action: actionId, command, started: now() });
      const count = counts.get(command) ?? { started: 0, completed: 0, failed: 0, slow: 0 };
      count.started++; counts.set(command, count);
      return ipc;
    },
    finish(ipc: number | undefined, failed: boolean, error?: unknown) {
      if (ipc === undefined) return;
      const request = pending.get(ipc); if (!request) return;
      pending.delete(ipc);
      const elapsed_ms = Math.max(0, now() - request.started);
      const count = counts.get(request.command) ?? { started: 0, completed: 0, failed: 0, slow: 0 };
      count.completed++; count.failed += Number(failed); count.slow += Number(elapsed_ms >= 1000);
      counts.set(request.command, count);
      if (failed || elapsed_ms >= 1000 || request.action !== null) {
        record({ ipc, action: request.action, command: request.command, elapsed_ms, failed,
          ...(failed ? { error: summarizeDiagnostic(error) } : {}) });
      }
    },
    snapshot() {
      const value = { counts: Object.fromEntries(counts), active_count: pending.size,
        pending: [...pending.values()].slice(0, 16).map(({ started, ...request }) => ({ ...request, elapsed_ms: Math.max(0, now() - started) })),
        recent, omitted };
      counts.clear(); recent = []; omitted = 0;
      return value;
    },
  };
}
const interactions = createInteractionObservations();
export function recordInteractionState(state: InteractionState): void {
  if (devDiagnosticsEnabled()) interactions.state(state);
}
export function traceInteractionAction(target: InteractionAction | undefined, run: () => void): void {
  if (!devDiagnosticsEnabled() || !target) { run(); return; }
  const action = interactions.action(target, "entered");
  logExploreDiagnostic(() => ({ event: "interaction", action, target, phase: "entered" }));
  try { run(); } finally {
    interactions.action(target, "returned");
    logExploreDiagnostic(() => ({ event: "interaction", action, target, phase: "returned" }));
  }
}

export function diagnosticInvoke<T>(invoke: (command: string, args?: Record<string, unknown>) => Promise<T>, command: string, args?: Record<string, unknown>): Promise<T> {
  if (!devDiagnosticsEnabled()) return invoke(command, args);
  const id = interactions.begin(command);
  const finish = (failed: boolean, error?: unknown) => interactions.finish(id, failed, error);
  try {
    return invoke(command, id === undefined ? args : { ...args, diagnosticId: id }).then(
      value => { finish(false); return value; }, error => { finish(true, error); throw error; });
  } catch (error) { finish(true, error); throw error; }
}

export function installInteractionProbe(artworkState?: () => { active: number; queued: number; inFlight: number; blocked: boolean }): () => void {
  if (!devDiagnosticsEnabled()) return () => {};
  let lastTick = performance.now(), maximumLag = 0, ticks = 0, lastReport = lastTick;
  let frame: number | undefined, frameStarted = lastTick, frameCompleted = 0;
  const input = (event: Event) => {
    const target = event.target instanceof Element ? event.target : null;
    interactions.input(event.type === "pointerdown" ? "pointer" : "keyboard",
      !!target?.closest("[data-sonante-sidebar]"), !!target?.closest('[role="dialog"], [aria-modal="true"]'));
  };
  document.addEventListener("pointerdown", input, true);
  document.addEventListener("keydown", input, true);
  const timer = window.setInterval(() => {
    const now = performance.now(); ticks++; maximumLag = Math.max(maximumLag, now - lastTick - 1000); lastTick = now;
    if (frame === undefined) {
      frameStarted = now;
      frame = requestAnimationFrame(() => { frame = undefined; frameCompleted++; });
    }
    if (now - lastReport >= 10_000) {
      let artwork_queue: object | null = null;
      if (artworkState) {
        try {
          const state = artworkState();
          const count = (n: number) => Number.isSafeInteger(n) && n >= 0 ? n : 0;
          artwork_queue = { active: count(state.active), queued: count(state.queued), in_flight: count(state.inFlight), blocked: !!state.blocked };
        } catch { /* Observations must not interfere with work. */ }
      }
      const sidebar = document.querySelector("[data-sonante-sidebar]");
      const rect = sidebar?.getBoundingClientRect();
      const hit = rect && rect.width > 0 && rect.height > 0
        ? document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2) : null;
      console.info("[Sonante] dev.frontend", JSON.stringify({ timestamp_ms: Date.now(), ...interactions.snapshot(),
        loop: { ticks, maximum_lag_ms: Math.round(maximumLag), frame_pending_ms: frame === undefined ? null : Math.round(now - frameStarted), frame_completed: frameCompleted, visible: document.visibilityState === "visible" },
        dom: { sidebar_hit: sidebar && hit ? sidebar.contains(hit) : null, sidebar_inert: !!sidebar?.closest("[inert]"),
          sidebar_pointer_blocked: !!sidebar && getComputedStyle(sidebar).pointerEvents === "none" },
        artwork_queue, explore_omitted: exploreOmitted, errors_omitted: suppressedErrors }));
      lastReport = now; ticks = 0; maximumLag = 0; exploreOmitted = 0; suppressedErrors = {};
    }
  }, 1000);
  return () => {
    window.clearInterval(timer); if (frame !== undefined) cancelAnimationFrame(frame);
    document.removeEventListener("pointerdown", input, true); document.removeEventListener("keydown", input, true);
  };
}
