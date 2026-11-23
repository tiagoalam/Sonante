import { parseLrc } from "./lrcParser.ts";

// Only whitelisted track metadata leaves the application; locators, paths,
// service URLs, credentials and audio never enter this contract.
export interface LyricsQuery { title: string; artist: string; album: string; duration: number | null }
export interface LyricsRecord {
  id: number; trackName: string; artistName: string; albumName: string; duration: number;
  instrumental: boolean; hasWordSync: boolean; plainLyrics: string | null; syncedLyrics: string | null;
}
type LyricsBackendFailure = "offline" | "timeout" | "rate_limited" | "service_unavailable" | "invalid_response";
export type LyricsLookupReply = { kind: "records"; records: LyricsRecord[]; persisted?: boolean } | { kind: "not_found" }
  | { kind: LyricsBackendFailure };
export type PersistedLyricsChoice = { kind: "automatic" } | { kind: "prefer_local" } | { kind: "manual"; record: LyricsRecord };
export const LYRICS_ENABLED_KEY = "sonante.nowPlaying.lyrics.enabled.v1";
export const LYRICS_PREFERENCE_EVENT = "sonante:lyrics-online-preference";
export function readLyricsOnlinePreference(): boolean {
  try { return window.localStorage.getItem(LYRICS_ENABLED_KEY) !== "false"; } catch { return true; }
}
export function writeLyricsOnlinePreference(enabled: boolean): void {
  try { window.localStorage.setItem(LYRICS_ENABLED_KEY, String(enabled)); } catch { /* Preference remains session-local. */ }
  if (typeof window !== "undefined") window.dispatchEvent(new Event(LYRICS_PREFERENCE_EVENT));
}
// v2 adds recording metadata and preserved synchronized LRC; v1 stored flattened text.
export const LYRICS_CACHE_KEY = "sonante.nowPlaying.lyrics.cache.v2";
const MAX_TEXT = 32_000, MAX_LEGACY_BYTES = 280_000, MAX_LEGACY_ENTRIES = 1_000;
const normalize = (value: string) => value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase("en-US");
const metadata = (value: unknown) => typeof value === "string" && value.length <= 400
  && !/[\u0000-\u001f\u007f]/.test(value) && !/https?:\/\/|file:|^\//i.test(value)
  && !/^[a-z][a-z0-9+.-]*:\/\//i.test(value) && !/^[a-z]:[\\/]/i.test(value)
  && !/^\\\\/.test(value) && !/(?:token|api[_-]?key|auth)=/i.test(value);

type LyricsMediaIdentity = { kind: "local"; uri: string; queue_index: number }
  | { kind: "plex"; server_id: string; part_key: string; queue_index: number };
export function hasSupportedLyricsIdentity(media: unknown): media is LyricsMediaIdentity {
  if (!media || typeof media !== "object") return false;
  const value = media as Record<string, unknown>;
  if (!Number.isSafeInteger(value.queue_index) || Number(value.queue_index) < 0) return false;
  if (value.kind === "local") return typeof value.uri === "string" && value.uri.length > 0 && value.uri.length <= 8_192;
  return value.kind === "plex" && typeof value.server_id === "string" && value.server_id.length > 0 && value.server_id.length <= 512
    && typeof value.part_key === "string" && value.part_key.length > 0 && value.part_key.length <= 2_048;
}
function validatedLyricsQuery(value: { title?: string; artist?: string; album?: string; duration?: number | null } | null | undefined): LyricsQuery | null {
  if (!value || !metadata(value.title) || !metadata(value.artist) || !value.title?.trim() || !value.artist?.trim()) return null;
  return { title: value.title.trim(), artist: value.artist.trim(), album: metadata(value.album) ? value.album!.trim() : "",
    duration: Number.isFinite(value.duration) && value.duration! >= 1 && value.duration! <= 3600 ? Math.round(value.duration!) : null };
}

/** Source identity gates online eligibility, but only whitelisted musical metadata is returned. */
export function lyricsQuery(track: ({ title?: string; artist?: string; album?: string; duration?: number; current_media?: unknown } | null | undefined)): LyricsQuery | null {
  return track && hasSupportedLyricsIdentity(track.current_media) ? validatedLyricsQuery(track) : null;
}
export const lyricsKey = (query: LyricsQuery) => JSON.stringify([normalize(query.title), normalize(query.artist), normalize(query.album), query.duration]);

function validRecord(value: unknown): value is LyricsRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return Number.isSafeInteger(record.id) && Number(record.id) > 0
    && metadata(record.trackName) && Boolean(record.trackName)
    && metadata(record.artistName) && Boolean(record.artistName)
    && metadata(record.albumName)
    && typeof record.duration === "number" && Number.isFinite(record.duration) && record.duration >= 1 && record.duration <= 3600
    && typeof record.instrumental === "boolean" && typeof record.hasWordSync === "boolean"
    && (record.plainLyrics === null || validLyricsText(record.plainLyrics))
    && (record.syncedLyrics === null || validLyricsText(record.syncedLyrics))
    && (record.instrumental || Boolean(record.plainLyrics?.toString().trim()) || Boolean(record.syncedLyrics?.toString().trim()));
}
function validLyricsText(value: unknown): value is string {
  return typeof value === "string" && value.length <= MAX_TEXT
    && !value.split("").some(char => /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(char));
}
function validateReply(value: unknown): LyricsLookupReply {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new LyricsLookupError("invalid_response");
  const reply = value as Record<string, unknown>;
  if (reply.kind === "not_found") return { kind: "not_found" };
  if (["offline", "timeout", "rate_limited", "service_unavailable", "invalid_response"].includes(String(reply.kind)))
    return { kind: reply.kind as LyricsBackendFailure };
  if (reply.kind !== "records" || !Array.isArray(reply.records) || reply.records.length > 20
    || !reply.records.every(validRecord) || new Set(reply.records.map(record => (record as LyricsRecord).id)).size !== reply.records.length)
    throw new LyricsLookupError("invalid_response");
  if (reply.persisted !== undefined && typeof reply.persisted !== "boolean") throw new LyricsLookupError("invalid_response");
  return { kind: "records", records: reply.records as LyricsRecord[], ...(typeof reply.persisted === "boolean" ? { persisted: reply.persisted } : {}) };
}
const RELEASE_EDITION = "(?:super\\s+)?(?:deluxe|expanded|anniversary|special)\\s+edition|remaster(?:ed)?(?:\\s+\\d{4})?|reissue";
function albumRecordingKey(value: string): string {
  const annotations = new RegExp(`\\s*[\\[(]\\s*(?:${RELEASE_EDITION})\\s*[\\])]`, "gi");
  const suffix = new RegExp(`(?:\\s*[-–:]\\s*|\\s+)(?:${RELEASE_EDITION})\\s*$`, "i");
  return normalize(value).replace(annotations, " ").replace(suffix, "").replace(/[\\s\\-–:]+$/g, "").trim();
}
const RECORDING_VARIANTS = ["live", "acoustic", "remix", "remixed", "karaoke", "demo", "instrumental"] as const;
function recordingVariants(value: string): string[] {
  const normalized = normalize(value);
  return RECORDING_VARIANTS.filter(variant => new RegExp(`(?:^|[^a-z])${variant}(?:$|[^a-z])`, "i").test(normalized));
}
function duplicateResultKey(record: LyricsRecord): string {
  return JSON.stringify([normalize(record.trackName), normalize(record.artistName), normalize(record.albumName),
    record.duration, record.instrumental, record.hasWordSync, record.plainLyrics, record.syncedLyrics]);
}
function lyricQuality(record: LyricsRecord): number {
  if (record.instrumental) return 2;
  if (record.syncedLyrics) {
    try { if (parseLrc(record.syncedLyrics).kind === "synchronized") return 0; } catch { /* A valid plain version may still be available. */ }
  }
  if (record.plainLyrics?.trim()) {
    try { parseLrc(record.plainLyrics); return 1; } catch { /* Invalid candidate is not auto-adopted. */ }
  }
  return 3;
}
/**
 * Results retain only clear title/artist matches, reject conflicting recording
 * qualifiers, and use album identity and duration to rank rather than demand
 * a single API result. Equal rankings are stable and independent of API order.
 * Synchronized lyrics require a known duration; simple text does not invent timing.
 */
export function automaticLyricsMatches(records: readonly LyricsRecord[], query: LyricsQuery): LyricsRecord[] {
  const queryAlbum = query.album.trim();
  const queryVariants = [...recordingVariants(query.title), ...recordingVariants(queryAlbum)];
  const eligible = records.filter(record => {
    if (lyricQuality(record) === 3 || normalize(record.trackName) !== normalize(query.title)
      || normalize(record.artistName) !== normalize(query.artist)) return false;
    if (query.duration === null && record.syncedLyrics && lyricQuality(record) === 0) return false;
    if (query.duration !== null && Math.abs(record.duration - query.duration) > 2) return false;
    const candidateVariants = [...recordingVariants(record.trackName), ...recordingVariants(record.albumName)];
    if (queryVariants.some(variant => !candidateVariants.includes(variant))
      || candidateVariants.some(variant => !queryVariants.includes(variant))) return false;
    return true;
  });
  const byContent = new Map<string, LyricsRecord>();
  for (const record of [...eligible].sort((a, b) => a.id - b.id)) {
    const key = duplicateResultKey(record);
    if (!byContent.has(key)) byContent.set(key, record);
  }
  const albumKey = queryAlbum ? albumRecordingKey(queryAlbum) : "";
  return [...byContent.values()].sort((a, b) => {
    const albumRank = (record: LyricsRecord) => !queryAlbum ? (record.albumName ? 1 : 2)
      : normalize(record.albumName) === normalize(queryAlbum) ? 0
        : albumRecordingKey(record.albumName) === albumKey && albumKey ? 1 : 2;
    const delta = (record: LyricsRecord) => query.duration === null ? 0 : Math.abs(record.duration - query.duration);
    return albumRank(a) - albumRank(b) || delta(a) - delta(b) || lyricQuality(a) - lyricQuality(b)
      || normalize(a.albumName).localeCompare(normalize(b.albumName), "en") || a.duration - b.duration || a.id - b.id;
  });
}

export type LyricsFailureReason = "offline" | "service" | "timeout" | "rate_limited" | "invalid_response";
export class LyricsLookupError extends Error {
  readonly reason: LyricsFailureReason;
  constructor(reason: LyricsFailureReason) { super("LYRICS_UNAVAILABLE"); this.name = "LyricsLookupError"; this.reason = reason; }
}
export function lyricsFailureReason(error: unknown): LyricsFailureReason {
  return error instanceof LyricsLookupError ? error.reason : "offline";
}
export type LegacyLyricsCacheEntry = { key: string; at: number; result: Extract<LyricsLookupReply, { kind: "records" | "not_found" }> };
type Job = { controller: AbortController; users: number; settled: boolean; promise: Promise<Extract<LyricsLookupReply, { kind: "records" | "not_found" }>> };

/** Persistent results belong to the backend. This client migrates the old
 * bounded localStorage cache and coalesces active consumers; it does not save
 * new results in WebView storage.
 */
export class LyricsClient {
  private jobs = new Map<string, Job>(); private loaded = false;
  private legacy = new Map<string, LegacyLyricsCacheEntry>(); private migration: Promise<boolean> | null = null;
  private request: (query: LyricsQuery, signal: AbortSignal, refresh: boolean) => Promise<unknown>;
  private migrate: (entries: LegacyLyricsCacheEntry[]) => Promise<boolean>;
  private storage: () => Storage | null; private now: () => number;
  constructor(request: (query: LyricsQuery, signal: AbortSignal, refresh: boolean) => Promise<unknown>, storage: () => Storage | null, now: () => number = Date.now,
    migrate: (entries: LegacyLyricsCacheEntry[]) => Promise<boolean> = async () => false) {
    this.request = request; this.storage = storage; this.now = now; this.migrate = migrate;
  }
  private load() {
    if (this.loaded) return; this.loaded = true;
    try {
      const raw = this.storage()?.getItem(LYRICS_CACHE_KEY); if (!raw || raw.length > MAX_LEGACY_BYTES) return;
      const values: unknown = JSON.parse(raw); if (!Array.isArray(values)) return;
      if (values.length > MAX_LEGACY_ENTRIES) return;
      for (const value of values) {
        try {
          if (!value || typeof value.key !== "string" || value.key.length > 2000 || !Number.isFinite(value.at) || !value.result) continue;
          const reply = validateReply(value.result);
          if (reply.kind !== "records" && reply.kind !== "not_found") continue;
          const entry: LegacyLyricsCacheEntry = { key: value.key, at: value.at as number, result: reply };
          if (reply.kind === "records" || reply.kind === "not_found" && entry.at <= this.now() && this.now() - entry.at < 3600_000)
            this.legacy.set(value.key, entry);
        } catch { /* One corrupt legacy entry must not hide other valid positive entries. */ }
      }
    } catch { /* Storage is optional; the online request remains usable. */ }
  }
  private migrateLegacy(): Promise<boolean> {
    this.load();
    if (this.migration) return this.migration;
    if (!this.legacy.size) return Promise.resolve(true);
    const entries = [...this.legacy.values()];
    this.migration = this.migrate(entries).then(ok => {
      if (ok) {
        try { this.storage()?.removeItem(LYRICS_CACHE_KEY); } catch { /* The imported copy is already durable. */ }
        this.legacy.clear();
      }
      return ok;
    }).catch(() => false).finally(() => { this.migration = null; });
    return this.migration;
  }
  async lookup(query: LyricsQuery, signal: AbortSignal, refresh = false): Promise<Extract<LyricsLookupReply, { kind: "records" | "not_found" }>> {
    if (signal.aborted) return Promise.reject(new DOMException("Canceled", "AbortError"));
    const checked = validatedLyricsQuery(query);
    if (!checked) return Promise.reject(new LyricsLookupError("invalid_response"));
    const key = lyricsKey(checked); this.load();
    const legacy = this.legacy.get(key);
    if (!refresh && legacy) { void this.migrateLegacy(); return legacy.result; }
    await this.migrateLegacy();
    if (signal.aborted) throw new DOMException("Canceled", "AbortError");
    const jobKey = `${key}:${refresh ? "refresh" : "cached"}`;
    let job = this.jobs.get(jobKey);
    if (!job || job.controller.signal.aborted) {
      const current: Job = { controller: new AbortController(), users: 0, settled: false, promise: Promise.resolve({ kind: "not_found" }) };
      current.promise = Promise.resolve().then(() => {
        if (current.controller.signal.aborted) throw new DOMException("Canceled", "AbortError");
        return this.request(checked, current.controller.signal, refresh);
      }).then(validateReply).then(reply => {
        if (reply.kind === "records" || reply.kind === "not_found") return reply;
        const reason = reply.kind === "offline" ? "offline" : reply.kind === "timeout" ? "timeout" : reply.kind === "rate_limited" ? "rate_limited" : reply.kind === "service_unavailable" ? "service" : "invalid_response";
        throw new LyricsLookupError(reason);
      });
      this.jobs.set(jobKey, current); job = current;
      void current.promise.then(() => {}, () => {}).then(() => { current.settled = true; if (this.jobs.get(jobKey) === current) this.jobs.delete(jobKey); });
    }
    const current = job; current.users++;
    return new Promise((resolve, reject) => {
      let done = false;
      const release = () => { if (done) return false; done = true; signal.removeEventListener("abort", onAbort); current.users--; if (!current.users && !current.settled) current.controller.abort(); return true; };
      const onAbort = () => { if (release()) reject(new DOMException("Canceled", "AbortError")); };
      signal.addEventListener("abort", onAbort, { once: true });
      current.promise.then(value => { if (release()) resolve(value); }, error => { if (release()) reject(error); });
    });
  }
}
