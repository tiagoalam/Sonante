import type { CurrentMedia, TrackMetadata, PlaybackStatus } from "../types/audio";

export const queueItemIsCurrent = (item: TrackMetadata, index: number, current: CurrentMedia | null): boolean => {
  if (!current || current.queue_index !== index) return false;
  if (current.kind === "plex") return item.media_locator?.kind === "plex" &&
    current.server_id === item.media_locator.server_id && current.part_key === item.media_locator.part_key;
  return (item.media_locator?.kind === "local" ? item.media_locator.uri : item.uri) === current.uri;
};

// Only decorate the exact current Local row; mixed/Plex identities and queue data stay intact.
export function queueItemArtwork(item: TrackMetadata, index: number, status?: PlaybackStatus): TrackMetadata {
  return status?.current_media?.kind === "local" && item.media_locator?.kind !== "plex"
    && !item.thumb && !item.plex_image && status.thumb && queueItemIsCurrent(item, index, status.current_media)
    ? { ...item, thumb: status.thumb } : item;
}

export interface QueuePreviewState {
  items: TrackMetadata[];
  phase: "idle" | "loading" | "ready" | "error";
}

// Read-only presentation of the existing logical queue. One request at a time;
// closing/unavailability invalidates a pending result, including rapid reopen.
export class QueuePreviewReader {
  private active = false;
  private epoch = 0;
  private running = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private state: QueuePreviewState = { items: [], phase: "idle" };
  private load: () => Promise<TrackMetadata[]>;
  private publish: (state: QueuePreviewState) => void;
  private interval: number;
  constructor(load: () => Promise<TrackMetadata[]>, publish: (state: QueuePreviewState) => void, interval = 2000) {
    this.load = load;
    this.publish = publish;
    this.interval = interval;
  }
  setActive(active: boolean) {
    if (active === this.active) return;
    this.active = active;
    this.epoch++;
    clearTimeout(this.timer);
    if (active) {
      this.state = { ...this.state, phase: "loading" };
      this.publish(this.state);
      void this.read();
    }
  }
  refresh(items?: TrackMetadata[]) {
    if (!this.active) return;
    this.epoch++;
    clearTimeout(this.timer);
    this.state = { items: items ?? this.state.items, phase: "loading" };
    this.publish(this.state);
    void this.read();
  }
  private async read() {
    if (!this.active || this.running) return;
    this.running = true;
    const epoch = this.epoch;
    try {
      const items = await this.load();
      if (this.active && epoch === this.epoch) {
        this.state = { items, phase: "ready" };
        this.publish(this.state);
      }
    } catch {
      if (this.active && epoch === this.epoch) {
        this.state = { ...this.state, phase: "error" };
        this.publish(this.state);
      }
    } finally {
      this.running = false;
      if (this.active) {
        if (epoch !== this.epoch) void this.read();
        else this.timer = setTimeout(() => void this.read(), this.interval);
      }
    }
  }
}

export const queueVisibleRange = (count: number, scrollTop: number, height: number) => {
  const start = Math.max(0, Math.min(Math.floor(scrollTop / 64) - 4, Math.max(0, count - 1)));
  return { start, end: Math.min(count, start + Math.ceil(height / 64) + 8) };
};

interface WorkArea { position: { x: number; y: number }; size: { width: number; height: number }; }
// All resulting creation options are logical pixels. Work-area inputs are physical.
export const nowPlayingWindowBounds = (area: WorkArea, scaleFactor: number) => {
  if (!Number.isFinite(scaleFactor) || scaleFactor <= 0 ||
    ![area.position.x, area.position.y, area.size.width, area.size.height].every(Number.isFinite) ||
    area.size.width <= 0 || area.size.height <= 0) throw new Error("Invalid monitor work area");
  const width = area.size.width / scaleFactor, height = area.size.height / scaleFactor;
  const margin = Math.min(24, width * 0.04, height * 0.04);
  // Reserve space for native decorations as well as the visible monitor margins.
  const innerWidth = Math.max(1, Math.floor(width - margin * 2 - 8));
  const innerHeight = Math.max(1, Math.floor(height - margin * 2 - 40));
  return { width: innerWidth, height: innerHeight,
    x: Math.round(area.position.x / scaleFactor + margin),
    y: Math.round(area.position.y / scaleFactor + margin),
    minWidth: Math.min(640, innerWidth), minHeight: Math.min(480, innerHeight) };
};

const trackIdentity = (item: TrackMetadata) => item.media_locator?.kind === "plex"
  ? JSON.stringify(["plex", item.media_locator.server_id, item.media_locator.part_key])
  : item.media_locator?.kind === "local" ? JSON.stringify(["local", item.media_locator.uri])
    : item.uri ? JSON.stringify(["local", item.uri]) : null;

export const playConfirmedQueuePosition = async (preview: TrackMetadata[], index: number,
  load: () => Promise<TrackMetadata[]>, play: (index: number) => Promise<void>, available: () => boolean) => {
  if (!available() || !preview[index] || !trackIdentity(preview[index])) throw new Error("Queue unavailable");
  const current = await load();
  // Titles are not identity; preserve real duplicates and their exact positions.
  if (!available() || current.length !== preview.length || current.some((item, i) => trackIdentity(item) !== trackIdentity(preview[i])))
    throw new Error("Queue changed");
  await play(index);
};
