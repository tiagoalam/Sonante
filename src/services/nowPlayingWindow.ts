import { emitTo } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

import type { NowPlayingSnapshot } from "../types/audio";

export const NOW_PLAYING_WINDOW_LABEL = "now-playing";
export const NOW_PLAYING_SNAPSHOT_EVENT = "now-playing://snapshot";
export const NOW_PLAYING_READY_EVENT = "now-playing://ready";

let openingWindow: Promise<void> | null = null;

const createNowPlayingWindow = async (title: string): Promise<void> => {
  const existing = await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL);
  if (existing) {
    await existing.setFocus();
    return;
  }

  await new Promise<void>((resolve, reject) => {
    const window = new WebviewWindow(NOW_PLAYING_WINDOW_LABEL, {
      url: "index.html?window=now-playing",
      title,
      width: 1100,
      height: 760,
      minWidth: 720,
      minHeight: 560,
      center: true,
      resizable: true,
      fullscreen: false,
    });

    void window.once("tauri://created", () => resolve());
    void window.once("tauri://error", (event) => reject(event.payload));
  });
};

export const openNowPlayingWindow = (title: string): Promise<void> => {
  if (!openingWindow) {
    openingWindow = createNowPlayingWindow(title).finally(() => {
      openingWindow = null;
    });
  }
  return openingWindow;
};

export const publishNowPlayingSnapshot = (snapshot: NowPlayingSnapshot): Promise<void> =>
  emitTo(NOW_PLAYING_WINDOW_LABEL, NOW_PLAYING_SNAPSHOT_EVENT, snapshot);
