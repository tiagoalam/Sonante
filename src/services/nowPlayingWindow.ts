import { emitTo } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

import type { NowPlayingSnapshot } from "../types/audio";

export const NOW_PLAYING_WINDOW_LABEL = "now-playing";
export const NOW_PLAYING_SNAPSHOT_EVENT = "now-playing://snapshot";
export const NOW_PLAYING_READY_EVENT = "now-playing://ready";
export const AUDIO_LEVEL_EVENT = "now-playing://audio-level";
export const ANALYZER_STATUS_EVENT = "now-playing://analyzer-status";

let openingWindow: Promise<void> | null = null;

const createNowPlayingWindow = async (title: string): Promise<void> => {
  const existing = await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL);
  if (existing) {
    await existing.setFocus();
    return;
  }

  const availableWidth = window.screen.availWidth || 1360;
  const availableHeight = window.screen.availHeight || 860;
  const width = Math.min(1360, Math.max(1, availableWidth - 64));
  const height = Math.min(860, Math.max(1, availableHeight - 64));

  await new Promise<void>((resolve, reject) => {
    const window = new WebviewWindow(NOW_PLAYING_WINDOW_LABEL, {
      url: "index.html?window=now-playing",
      title,
      width,
      height,
      minWidth: Math.min(1024, width),
      minHeight: Math.min(700, height),
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

let analyzerTransition: Promise<void> = Promise.resolve();

// Mantém start/stop na ordem do mesmo webview, inclusive no replay de efeitos do StrictMode.
const queueAnalyzerTransition = (action: () => Promise<void>): Promise<void> => {
  const next = analyzerTransition.catch(() => {}).then(action);
  analyzerTransition = next;
  return next;
};

export const startAudioAnalyzer = (sessionId: string): Promise<void> =>
  queueAnalyzerTransition(() => invoke("start_audio_analyzer", { sessionId }));

export const stopAudioAnalyzer = (sessionId: string): Promise<void> =>
  queueAnalyzerTransition(() => invoke("stop_audio_analyzer", { sessionId }));
