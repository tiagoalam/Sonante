export const NOW_PLAYING_LAYOUTS = ["audiophile", "psycho", "sleeve", "poster"] as const;
export type NowPlayingLayout = typeof NOW_PLAYING_LAYOUTS[number];
export type PsychoPlacement = "frame" | "background";
export interface NowPlayingLook { layout: NowPlayingLayout; psychoPlacement: PsychoPlacement; }
export const NOW_PLAYING_LOOK_KEY = "sonante.nowPlaying.look.v1";

export const nowPlayingLayout = (value: unknown): NowPlayingLayout =>
  NOW_PLAYING_LAYOUTS.find(layout => layout === value) ?? "audiophile";

export function parseNowPlayingLook(value: string | null): NowPlayingLook {
  try {
    const data: unknown = value ? JSON.parse(value) : null;
    if (data && typeof data === "object") {
      const { layout, psychoPlacement } = data as Partial<NowPlayingLook>;
      return { layout: nowPlayingLayout(layout), psychoPlacement: psychoPlacement === "background" ? "background" : "frame" };
    }
  } catch { /* A damaged visual preference never blocks the player. */ }
  return { layout: "audiophile", psychoPlacement: "frame" };
}
export function readNowPlayingLook(): NowPlayingLook {
  try { return parseNowPlayingLook(window.localStorage.getItem(NOW_PLAYING_LOOK_KEY)); }
  catch { return parseNowPlayingLook(null); }
}
// Presentation only: no profile/config transaction, queue or audio command.
export function saveNowPlayingLook(look: NowPlayingLook): void {
  try { window.localStorage.setItem(NOW_PLAYING_LOOK_KEY, JSON.stringify(look)); }
  catch { /* The selected look remains usable for this window's session. */ }
}

export const NOW_PLAYING_QUEUE_PANELS_KEY = "sonante.nowPlaying.queuePanels.v1";
export type NowPlayingQueuePanels = Record<NowPlayingLayout, boolean>;
export function normalizeQueuePanels(value: unknown): NowPlayingQueuePanels {
  const data = value && typeof value === "object" ? value as Partial<NowPlayingQueuePanels> : {};
  return { audiophile: typeof data.audiophile === "boolean" ? data.audiophile : true,
    psycho: typeof data.psycho === "boolean" ? data.psycho : true,
    sleeve: typeof data.sleeve === "boolean" ? data.sleeve : true,
    poster: typeof data.poster === "boolean" ? data.poster : true };
}
export function parseQueuePanels(value: string | null): NowPlayingQueuePanels {
  try { return normalizeQueuePanels(value ? JSON.parse(value) : null); }
  catch { return normalizeQueuePanels(null); }
}
export function readQueuePanels(): NowPlayingQueuePanels {
  try { return parseQueuePanels(window.localStorage.getItem(NOW_PLAYING_QUEUE_PANELS_KEY)); }
  catch { return normalizeQueuePanels(null); }
}
export function saveQueuePanels(value: NowPlayingQueuePanels): void {
  try { window.localStorage.setItem(NOW_PLAYING_QUEUE_PANELS_KEY, JSON.stringify(value)); }
  catch { /* Panel visibility remains usable for this session. */ }
}
