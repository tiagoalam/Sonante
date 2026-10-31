// Plain lyrics only. No media locator, token, local path or audio data enters a request.
export interface LyricsQuery { title: string; artist: string; album: string; duration: number | null }
export type LyricsResult = { kind: "found"; text: string } | { kind: "not_found" | "instrumental" };
export const LYRICS_ENABLED_KEY = "sonante.nowPlaying.lyrics.enabled.v1";
export const LYRICS_CACHE_KEY = "sonante.nowPlaying.lyrics.cache.v1";
const MAX_TEXT = 32_000, MAX_BODY = 256 * 1024, MAX_CACHE = 280_000, MAX_ENTRIES = 20;
const normalize = (value: string) => value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase("en-US");
const metadata = (value: unknown) => typeof value === "string" && value.length <= 400 && !/[\u0000-\u001f\u007f]/.test(value) && !/https?:\/\/|file:|^\//i.test(value);
export function lyricsQuery(track: { title?: string; artist?: string; album?: string; duration?: number; current_media?: { kind: string } | null } | null | undefined): LyricsQuery | null {
  if (!track || track.current_media?.kind === "radio" || !metadata(track.title) || !metadata(track.artist) || !track.title?.trim() || !track.artist?.trim()) return null;
  return { title: track.title.trim(), artist: track.artist.trim(), album: metadata(track.album) ? track.album!.trim() : "",
    duration: Number.isFinite(track.duration) && track.duration! >= 1 && track.duration! <= 3600 ? Math.round(track.duration!) : null };
}
export const lyricsKey = (query: LyricsQuery) => JSON.stringify([normalize(query.title), normalize(query.artist), normalize(query.album), query.duration]);
export function parseLyrics(value: unknown, query: LyricsQuery): LyricsResult {
  if (!value || typeof value !== "object") throw Error("LYRICS_UNAVAILABLE");
  const record = value as Record<string, unknown>;
  if (typeof record.trackName !== "string" || typeof record.artistName !== "string"
    || normalize(record.trackName) !== normalize(query.title) || normalize(record.artistName) !== normalize(query.artist)
    || (query.duration !== null && (typeof record.duration !== "number" || !Number.isFinite(record.duration) || Math.abs(record.duration - query.duration) > 2))) return { kind: "not_found" };
  if (record.instrumental === true) return { kind: "instrumental" };
  let text = typeof record.plainLyrics === "string" ? record.plainLyrics.trim() : "";
  if (!text && typeof record.syncedLyrics === "string") text = record.syncedLyrics.split(/\r?\n/).map(line => line.replace(/^(?:\[\d+:\d+(?:\.\d+)?\])+\s*/, "")).filter(line => !/^\[[a-z]+:/i.test(line)).join("\n").trim();
  if (!text) return { kind: "not_found" };
  if (text.length > MAX_TEXT || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(text)) throw Error("LYRICS_UNAVAILABLE");
  return { kind: "found", text };
}
export type LyricsFailureReason = "connection" | "service" | "timeout" | "invalid_response";
export class LyricsLookupError extends Error {
  readonly reason: LyricsFailureReason;
  readonly status?: number;
  constructor(reason: LyricsFailureReason, status?: number) {
    super("LYRICS_UNAVAILABLE"); this.name = "LyricsLookupError"; this.reason = reason; this.status = status;
  }
}
export function lyricsFailureReason(error: unknown): LyricsFailureReason {
  return error instanceof LyricsLookupError ? error.reason : "connection";
}
const aborted = () => new DOMException("Canceled", "AbortError");
function delay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(aborted()); return; }
    const onAbort = () => { clearTimeout(timer); signal.removeEventListener("abort", onAbort); reject(aborted()); };
    const timer = setTimeout(() => { signal.removeEventListener("abort", onAbort); resolve(); }, ms);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}
type CacheEntry = { key: string; at: number; result: LyricsResult };
type Job = { controller: AbortController; users: number; settled: boolean; promise: Promise<LyricsResult> };
export class LyricsClient {
  private entries = new Map<string, CacheEntry>();
  private jobs = new Map<string, Job>();
  private loaded = false;
  private serial: Promise<unknown> = Promise.resolve();
  private nextRequest = 0;
  private cooldown = 0;
  private request: typeof fetch;
  private storage: () => Storage | null;
  private now: () => number;
  constructor(request: typeof fetch, storage: () => Storage | null, now: () => number = Date.now) { this.request = request; this.storage = storage; this.now = now; }
  private valid(entry: CacheEntry): boolean { return entry.at <= this.now() && this.now() - entry.at < (entry.result.kind === "not_found" ? 3600_000 : 30 * 86400_000); }
  private load() {
    if (this.loaded) return; this.loaded = true;
    try {
      const raw = this.storage()?.getItem(LYRICS_CACHE_KEY);
      if (!raw || raw.length > MAX_CACHE) return;
      const values: unknown = JSON.parse(raw);
      if (!Array.isArray(values)) return;
      for (const value of values.slice(-MAX_ENTRIES)) {
        if (!value || typeof value.key !== "string" || value.key.length > 2000 || !Number.isFinite(value.at) || !value.result) continue;
        const result = value.result;
        if (!(result.kind === "not_found" || result.kind === "instrumental" || result.kind === "found" && typeof result.text === "string" && result.text.length <= MAX_TEXT)) continue;
        const entry: CacheEntry = { key: value.key, at: value.at, result: result.kind === "found" ? { kind: "found", text: result.text } : { kind: result.kind } };
        if (this.valid(entry)) this.entries.set(entry.key, entry);
      }
    } catch { /* Storage is optional; session cache still works. */ }
  }
  private save(key: string, result: LyricsResult) {
    this.entries.delete(key); this.entries.set(key, { key, result, at: this.now() });
    for (const [key, entry] of this.entries) if (!this.valid(entry)) this.entries.delete(key);
    let raw = JSON.stringify([...this.entries.values()]);
    while (this.entries.size > MAX_ENTRIES || raw.length > MAX_CACHE) {
      this.entries.delete(this.entries.keys().next().value!); raw = JSON.stringify([...this.entries.values()]);
    }
    try { this.storage()?.setItem(LYRICS_CACHE_KEY, raw); } catch { /* Quota/blocked storage never affects playback. */ }
  }
  lookup(query: LyricsQuery, signal: AbortSignal): Promise<LyricsResult> {
    if (signal.aborted) return Promise.reject(aborted());
    // Validate again at the public service boundary, including any direct caller.
    const checked = lyricsQuery({ ...query, duration: query.duration ?? undefined });
    if (!checked) return Promise.reject(Error("LYRICS_UNAVAILABLE"));
    query = checked; const key = lyricsKey(query); this.load();
    const cached = this.entries.get(key);
    if (cached && this.valid(cached)) return Promise.resolve(cached.result);
    let job = this.jobs.get(key);
    if (!job || job.controller.signal.aborted) {
      const current: Job = { controller: new AbortController(), users: 0, settled: false, promise: Promise.resolve({ kind: "not_found" }) };
      current.promise = this.serial.catch(() => {}).then(() => this.fetch(query, current.controller));
      this.serial = current.promise.then(() => {}, () => {});
      this.jobs.set(key, current); job = current;
      void current.promise.then(result => { this.save(key, result); }, () => {}).then(() => {
        current.settled = true; if (this.jobs.get(key) === current) this.jobs.delete(key);
      });
    }
    const current = job; current.users++;
    return new Promise((resolve, reject) => {
      let done = false;
      const release = () => { if (done) return false; done = true; signal.removeEventListener("abort", onAbort); current.users--; if (!current.users && !current.settled) current.controller.abort(); return true; };
      const onAbort = () => { if (release()) reject(aborted()); };
      signal.addEventListener("abort", onAbort, { once: true });
      current.promise.then(value => { if (release()) resolve(value); }, error => { if (release()) reject(error); });
    });
  }
  private async fetch(query: LyricsQuery, controller: AbortController): Promise<LyricsResult> {
    const signal = controller.signal;
    if (signal.aborted) throw aborted();
    if (this.cooldown > this.now()) throw new LyricsLookupError("service");
    if (this.nextRequest > this.now()) await delay(this.nextRequest - this.now(), signal);
    if (signal.aborted) throw aborted();
    this.nextRequest = this.now() + 350;
    const url = new URL("https://lrclib.net/api/get");
    url.searchParams.set("track_name", query.title); url.searchParams.set("artist_name", query.artist);
    if (query.album) url.searchParams.set("album_name", query.album);
    if (query.duration !== null) url.searchParams.set("duration", String(query.duration));
    let timedOut = false;
    const timeout = setTimeout(() => { timedOut = true; controller.abort(); }, 10_000);
    try {
      const response = await this.request(url.toString(), { signal, credentials: "omit", redirect: "error", headers: { "X-User-Agent": "Sonante v0.5.9 (https://github.com/tiagoalam/Sonante)", Accept: "application/json" } });
      if (response.status === 429 || response.status >= 500) {
        const retry = response.headers.get("Retry-After") ?? "";
        const seconds = /^\d+$/.test(retry) ? Number(retry) : Math.max(0, (Date.parse(retry) - this.now()) / 1000);
        this.cooldown = this.now() + Math.max(60, Number.isFinite(seconds) ? seconds : 60) * 1000 + Math.random() * 1000;
        throw new LyricsLookupError("service", response.status);
      }
      if (response.status === 404) return { kind: "not_found" };
      if (!response.ok) throw new LyricsLookupError("service", response.status);
      if (!response.headers.get("Content-Type")?.includes("application/json") || Number(response.headers.get("Content-Length")) > MAX_BODY) throw new LyricsLookupError("invalid_response", response.status);
      const reader = response.body?.getReader();
      if (!reader) throw new LyricsLookupError("invalid_response");
      const chunks: Uint8Array[] = []; let length = 0;
      try {
        for (;;) {
          const { done, value } = await reader.read(); if (done) break;
          length += value.length; if (length > MAX_BODY) throw new LyricsLookupError("invalid_response"); chunks.push(value);
        }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      const bytes = new Uint8Array(length); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      if (signal.aborted) throw aborted();
      try { return parseLyrics(JSON.parse(new TextDecoder().decode(bytes)), query); }
      catch { throw new LyricsLookupError("invalid_response"); }
    } catch (error) {
      if (timedOut) throw new LyricsLookupError("timeout");
      if (signal.aborted) throw aborted();
      throw error instanceof LyricsLookupError ? error : new LyricsLookupError("connection");
    } finally { clearTimeout(timeout); }
  }
}
export const trackLyrics = new LyricsClient((...args) => fetch(...args), () => { try { return window.localStorage; } catch { return null; } });
