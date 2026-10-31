import type { CurrentMedia, PlaybackStatus, TrackMetadata } from "../types/audio";
import { normalizeLocalNavigationText } from "./localAlbumNavigation.ts";
import { queueItemIsCurrent } from "./nowPlayingPresentation.ts";

export interface QueueEntry { item: TrackMetadata; index: number }
export interface QueueTime { seconds: number; unknown: number }
const duration = (value: number | undefined) => Number.isFinite(value) && value! > 0 ? value! : null;

export function confirmedQueueIndex(items: readonly TrackMetadata[], current: CurrentMedia | null | undefined): number | null {
  const index = current?.queue_index;
  return index !== undefined && Number.isInteger(index) && index >= 0 && index < items.length
    && queueItemIsCurrent(items[index], index, current ?? null) ? index : null;
}

// Index metadata once per queue snapshot. Filtering never changes occurrence
// indices; duration suffixes avoid a full scan on every playback-status tick.
export function queuePresentation(items: readonly TrackMetadata[]) {
  const entries: QueueEntry[] = items.map((item, index) => ({ item, index }));
  const text = items.map(item => normalizeLocalNavigationText([item.title, item.artist, item.album].filter(Boolean).join(" ")));
  const seconds = new Float64Array(items.length + 1), unknown = new Uint32Array(items.length + 1);
  for (let index = items.length - 1; index >= 0; index--) {
    const value = duration(items[index].duration);
    seconds[index] = seconds[index + 1] + (value ?? 0);
    unknown[index] = unknown[index + 1] + (value === null ? 1 : 0);
  }
  return {
    identity: JSON.stringify(items.map(item => item.media_locator ?? { kind: "local", uri: item.uri ?? "" })),
    entries,
    total: { seconds: seconds[0], unknown: unknown[0] } as QueueTime,
    filter(query: string): QueueEntry[] {
      const terms = normalizeLocalNavigationText(query).split(/\s+/).filter(Boolean);
      return terms.length ? entries.filter(entry => terms.every(term => text[entry.index].includes(term))) : entries;
    },
    remaining(status: PlaybackStatus | undefined, available: boolean): QueueTime | null {
      if (!available || !status || status.snapshot_consistent === false || !status.options || status.options.random
        || status.options.repeat || status.options.single !== "off" || !Number.isFinite(status.elapsed) || status.elapsed < 0) return null;
      const index = confirmedQueueIndex(items, status.current_media);
      if (index === null) return null;
      const currentDuration = duration(status.duration) ?? duration(items[index].duration);
      return { seconds: seconds[index + 1] + (currentDuration === null ? 0 : Math.max(0, currentDuration - status.elapsed)),
        unknown: unknown[index + 1] + (currentDuration === null ? 1 : 0) };
    },
  };
}

export function formatQueueTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const value = Math.floor(seconds), minutes = Math.floor(value / 60), remainder = String(value % 60).padStart(2, "0");
  return minutes >= 60 ? `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}:${remainder}` : `${minutes}:${remainder}`;
}
