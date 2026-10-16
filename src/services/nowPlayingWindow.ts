import { emitTo, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { logError } from "../utils/frontendDiagnostics";
import { nowPlayingWindowBounds } from "../utils/nowPlayingPresentation";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

import type { AnalyzerOwner, NowPlayingSnapshot } from "../types/audio";

export const NOW_PLAYING_WINDOW_LABEL = "now-playing";
export const NOW_PLAYING_SNAPSHOT_EVENT = "now-playing://snapshot";
export const NOW_PLAYING_READY_EVENT = "now-playing://ready";
export const AUDIO_LEVEL_EVENT = "now-playing://audio-level";
export const ANALYZER_STATUS_EVENT = "now-playing://analyzer-status";

export const HOST_REQUEST_EVENT = "now-playing://host-request";
export const HOST_REPLY_EVENT = "now-playing://host-reply";
export const REINCORPORATE_EVENT = "now-playing://reincorporate";
export const NAVIGATE_EVENT = "now-playing://navigate";
let openingWindow: Promise<string> | null = null;
let hostTransition = 0;
const closeListeners = new Set<() => void>();
const watched = new Set<string>();
export const onDetachedWindowClosed = (fn: () => void) => { closeListeners.add(fn); return () => { closeListeners.delete(fn); }; };


const createNowPlayingWindow = async (title: string): Promise<void> => {
  const existing = await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL);
  if (existing) {
    await existing.setFocus();
    return;
  }

  // Browser screen dimensions are logical CSS pixels; only a fallback if the
  // native monitor cannot be read. Keep even this initial size within that area.
  const fallbackWidth = Math.max(1, Math.floor(window.screen.availWidth * 0.92 - 8));
  const fallbackHeight = Math.max(1, Math.floor(window.screen.availHeight * 0.92 - 40));
  let bounds = { width: fallbackWidth, height: fallbackHeight, minWidth: Math.min(640, fallbackWidth),
    minHeight: Math.min(480, fallbackHeight), center: true } as Record<string, number | boolean>;
  try {
    const monitor = await currentMonitor();
    if (monitor) bounds = nowPlayingWindowBounds(monitor.workArea, monitor.scaleFactor);
  } catch (err) {
    logError("now-playing.open", err);
  }

  await new Promise<void>((resolve, reject) => {
    let settled = false;
    const cleanups: Array<() => void> = [];
    const timeout = setTimeout(() => finish(new Error("Window creation timeout")), 8000);
    const finish = (error?: unknown) => {
      if (settled) return;
      settled = true; clearTimeout(timeout); cleanups.forEach(fn => fn());
      if (error) reject(error); else resolve();
    };
    const view = new WebviewWindow(NOW_PLAYING_WINDOW_LABEL, {
      url: "index.html?window=now-playing", title, ...bounds, resizable: true, fullscreen: false,
    });
    const keep = (cleanup: () => void) => { if (settled) cleanup(); else cleanups.push(cleanup); };
    void view.once("tauri://created", () => finish()).then(keep).catch(finish);
    void view.once("tauri://error", event => finish(event.payload)).then(keep).catch(finish);
  });
};

export const exchangeDetachedHost = async (action: "ready" | "activate" | "suspend", runtime?: string, owner?: AnalyzerOwner): Promise<string> => {
  const requestId = window.crypto.randomUUID();
  const transition = ++hostTransition;
  let finished = false;
  let unlisten: (() => void) | undefined;
  let poll: ReturnType<typeof setInterval> | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await new Promise<string>((resolve, reject) => {
      const send = async () => {
        try {
          if (!await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL)) throw new Error("Detached window closed");
          await emitTo(NOW_PLAYING_WINDOW_LABEL, HOST_REQUEST_EVENT, { action, requestId, runtime, owner, transition });
        } catch (error) { reject(error); }
      };
      timeout = setTimeout(() => reject(new Error("Detached host readiness timeout")), 8000);
      void listen<{ requestId: string; runtime: string; ok: boolean }>(HOST_REPLY_EVENT, event => {
        if (event.payload.requestId !== requestId || runtime && event.payload.runtime !== runtime) return;
        if (event.payload.ok) resolve(event.payload.runtime);
        else reject(new Error("Detached host refused transition"));
      }).then(cleanup => { if (finished) { cleanup(); return; } unlisten = cleanup; poll = setInterval(() => void send(), 250); void send(); }).catch(reject);
    });
  } finally { finished = true; unlisten?.(); clearTimeout(timeout); clearInterval(poll); }
};

export const openNowPlayingWindow = (title: string): Promise<string> => {
  if (!openingWindow) openingWindow = (async () => {
    await createNowPlayingWindow(title);
    const runtime = await exchangeDetachedHost("ready");
    if (!watched.has(runtime)) {
      watched.add(runtime);
      const view = await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL);
      if (!view) { watched.delete(runtime); throw new Error("Detached window closed"); }
      try { await view.once("tauri://destroyed", () => { watched.delete(runtime); closeListeners.forEach(fn => fn()); }); }
      catch (error) { watched.delete(runtime); throw error; }
    }
    return runtime;
  })().finally(() => { openingWindow = null; });
  return openingWindow;
};

export const closeDetachedWindow = async () => {
  const view = await WebviewWindow.getByLabel(NOW_PLAYING_WINDOW_LABEL);
  if (!view) return;
  let cleanup: (() => void) | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    let resolveDestroyed: () => void = () => {};
    const destroyed = new Promise<void>((resolve, reject) => {
      resolveDestroyed = resolve;
      timeout = setTimeout(() => reject(new Error("Detached close timeout")), 8000);
    });
    cleanup = await view.once("tauri://destroyed", resolveDestroyed);
    await view.close();
    await destroyed;
    await getCurrentWindow().setFocus();
  } finally { cleanup?.(); clearTimeout(timeout); }
};

export const claimAudioAnalyzerHost = (host: AnalyzerOwner["host"], sessionId: string): Promise<AnalyzerOwner> =>
  invoke("claim_audio_analyzer_host", { host, sessionId });

export const publishNowPlayingSnapshot = (snapshot: NowPlayingSnapshot): Promise<void> =>
  emitTo(NOW_PLAYING_WINDOW_LABEL, NOW_PLAYING_SNAPSHOT_EVENT, snapshot);

let analyzerTransition: Promise<void> = Promise.resolve();

// Mantém start/stop na ordem do mesmo webview, inclusive no replay de efeitos do StrictMode.
const queueAnalyzerTransition = (action: () => Promise<void>): Promise<void> => {
  const next = analyzerTransition.catch(() => {}).then(action);
  analyzerTransition = next;
  return next;
};

export const startAudioAnalyzer = (owner: AnalyzerOwner, leaseId: string): Promise<void> =>
  queueAnalyzerTransition(() => invoke("start_audio_analyzer", { sessionId: owner.sessionId, generation: owner.generation, leaseId }));

export const stopAudioAnalyzer = (owner: AnalyzerOwner, leaseId: string): Promise<void> =>
  queueAnalyzerTransition(() => invoke("stop_audio_analyzer", { sessionId: owner.sessionId, generation: owner.generation, leaseId }));

export const releaseAudioAnalyzerHost = (owner: AnalyzerOwner): Promise<void> =>
  invoke("release_audio_analyzer_host", { sessionId: owner.sessionId, generation: owner.generation });
