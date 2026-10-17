import type { CurrentMedia } from "../types/audio";
import type { PlayerSourceInfo, SourceAudioFacts } from "../types/sourceAudio";
import { currentMediaKey } from "./playerNavigation.ts";

const families: Record<string, "lossless" | "lossy" | "pcm" | "dsd" | "other"> = {
  FLAC: "lossless", ALAC: "lossless", MP3: "lossy", AAC: "lossy", Vorbis: "lossy", Opus: "lossy",
  Musepack: "lossy", PCM: "pcm", "PCM float": "pcm", DSD: "dsd", WavPack: "other",
};
const positiveInt = (n: unknown, max: number): n is number => typeof n === "number" && Number.isSafeInteger(n) && n > 0 && n <= max;
export function sourceAudioBadge(info: SourceAudioFacts | null | undefined) {
  if (!info || !["local", "plex"].includes(info.provenance) || !Object.prototype.hasOwnProperty.call(families, info.codec)) return null;
  const family = families[info.codec];
  const rate = positiveInt(info.sample_rate, 50_000_000) ? info.sample_rate : null;
  const depth = positiveInt(info.bit_depth, 64) ? info.bit_depth : null;
  const dsd = info.codec === "DSD" && info.sample_type === "dsd" && [64,128,256].includes(info.dsd_level ?? 0)
    && rate === 44_100 * (info.dsd_level ?? 0) ? info.dsd_level : null;
  const hiRes = dsd !== null || (info.sample_type === "integer" && ["lossless", "uncompressed"].includes(info.compression)
    && family !== "lossy" && family !== "dsd" && depth !== null && depth >= 24 && rate !== null && rate >= 96_000);
  return { label: dsd ? `DSD${dsd}` : info.codec, family, hiRes, rate, depth };
}

// One current result (including a negative result), one physical acquisition and
// one replaceable desired identity. No queue growth, polling, timers or persistence.
export class PlayerSourceReader {
  private desired: { media: CurrentMedia; context: object; key: string; generation: number } | null = null;
  private generation = 0;
  private pending = false;
  private settled = -1;
  private snapshot: PlayerSourceInfo | null = null;
  private listeners = new Set<() => void>();
  private readonly read: (media: CurrentMedia, current: () => boolean) => Promise<PlayerSourceInfo | null>;
  constructor(read: (media: CurrentMedia, current: () => boolean) => Promise<PlayerSourceInfo | null>) { this.read = read; }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.snapshot;
  private publish(value: PlayerSourceInfo | null) { if (this.snapshot !== value) { this.snapshot = value; this.listeners.forEach(fn => fn()); } }
  setContext(media: CurrentMedia | null, context: object) {
    const key = currentMediaKey(media);
    if (this.desired?.key === key && this.desired.context === context) return;
    this.generation++;
    this.desired = media ? { media, context, key, generation: this.generation } : null;
    this.publish(null); this.pump();
  }
  matches(media: CurrentMedia | null, context: object) { return this.desired?.key === currentMediaKey(media) && this.desired.context === context; }
  private pump() {
    const job = this.desired;
    if (!job || this.pending || this.settled === job.generation) return;
    this.pending = true;
    void Promise.resolve().then(() => this.desired === job ? this.read(job.media, () => this.desired === job) : null).then(result => {
      if (this.desired !== job) return;
      if (result && currentMediaKey(result.media) === job.key && result.info.provenance === job.media.kind && sourceAudioBadge(result.info)) this.publish(result);
    }).catch(() => { /* Optional source facts never drive playback/recovery/logging. */ }).finally(() => {
      this.settled = job.generation; this.pending = false; this.pump();
    });
  }
}

// Only a complete, bounded album inventory can describe an album.
export function albumAudioBadge(facts: readonly SourceAudioFacts[] | null | undefined) {
  if (!facts?.length || facts.length > 32) return null;
  const badges = facts.map(sourceAudioBadge);
  if (badges.some(badge => !badge) || facts.some(info => info.provenance !== facts[0].provenance)) return null;
  const known = badges.filter((badge): badge is NonNullable<typeof badge> => !!badge);
  const same = known.every(badge => badge.label === known[0].label);
  return { label: same ? known[0].label : "mixed", family: same ? known[0].family : "other" as const,
    hiRes: known.every(badge => badge.hiRes), rate: null, depth: null };
}
