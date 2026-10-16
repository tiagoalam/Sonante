import React, { useEffect, useRef, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import type { AnalyzerOwner, NowPlayingSnapshot } from "../types/audio";
import { NowPlayingHost } from "./NowPlayingHost";
import { analyzerOwnerMatches } from "../utils/nowPlayingHosting";
import { logError } from "../utils/frontendDiagnostics";
import { HOST_REQUEST_EVENT, HOST_REPLY_EVENT, NAVIGATE_EVENT, NOW_PLAYING_READY_EVENT, NOW_PLAYING_SNAPSHOT_EVENT, REINCORPORATE_EVENT } from "../services/nowPlayingWindow";

export const NowPlayingWindow: React.FC = () => {
  const runtime = useRef(window.crypto.randomUUID()).current;
  const [snapshot, setSnapshot] = useState<NowPlayingSnapshot | null>(null);
  const snapshotRef = useRef<NowPlayingSnapshot | null>(null);
  const [owner, setOwner] = useState<AnalyzerOwner | null>(null);
  const currentOwner = useRef<AnalyzerOwner | null>(null);
  currentOwner.current = owner;
  const [waiting, setWaiting] = useState(false);
  const lastTransition = useRef(0);
  const readyOwner = useRef<AnalyzerOwner | null>(null);
  const requests = useRef(new Map<string, AnalyzerOwner>());
  const reply = (requestId: string, ok: boolean) => emitTo("main", HOST_REPLY_EVENT, { requestId, runtime, ok }).catch(err => logError("now-playing.open", err));
  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];
    const register = async <T,>(event: string, handler: (data: T) => void) => {
      const cleanup = await listen<T>(event, data => { if (!disposed) handler(data.payload); });
      if (disposed) cleanup(); else cleanups.push(cleanup);
    };
    void Promise.all([
      register<NowPlayingSnapshot>(NOW_PLAYING_SNAPSHOT_EVENT, data => { snapshotRef.current = data; setSnapshot(data); if (readyOwner.current) ready(readyOwner.current); }),
      register<{ action: string; requestId: string; runtime?: string; owner?: AnalyzerOwner; transition: number }>(HOST_REQUEST_EVENT, data => {
        if (data.runtime && data.runtime !== runtime) { void reply(data.requestId, false); return; }
        if (data.action === "ready") { void reply(data.requestId, true); return; }
        if (!Number.isSafeInteger(data.transition) || data.transition <= 0 || data.transition < lastTransition.current) { void reply(data.requestId, false); return; }
        lastTransition.current = data.transition;
        if (data.action === "suspend") {
          requests.current.clear(); readyOwner.current = null; currentOwner.current = null; setOwner(null); setWaiting(true); void reply(data.requestId, true); return;
        }
        if (data.action === "activate" && data.owner?.host === "now-playing") {
          if (analyzerOwnerMatches(readyOwner.current, data.owner)) void reply(data.requestId, true);
          else { requests.current.set(data.requestId, data.owner); currentOwner.current = data.owner; setOwner(data.owner); setWaiting(false); }
        } else void reply(data.requestId, false);
      }),
    ]).then(() => { if (!disposed) return emitTo("main", NOW_PLAYING_READY_EVENT, { runtime }); })
      .catch(err => { if (!disposed) logError("now-playing.open", err); });
    return () => { disposed = true; cleanups.forEach(fn => fn()); requests.current.clear(); };
  }, [runtime]);
  const ready = (next: AnalyzerOwner) => {
    if (!analyzerOwnerMatches(currentOwner.current, next)) return;
    readyOwner.current = next;
    if (!snapshotRef.current) return;
    for (const [id, expected] of requests.current) if (analyzerOwnerMatches(next, expected)) {
      requests.current.delete(id); void reply(id, true);
    }
  };
  return <NowPlayingHost snapshot={snapshot} owner={owner} embedded={false} busy={waiting} onReady={ready}
    onReincorporate={() => { if (waiting || !owner) return; setWaiting(true); void emitTo("main", REINCORPORATE_EVENT, { runtime }).catch(err => { setWaiting(false); logError("now-playing.open", err); }); }}
    onNavigate={view => { if (!waiting) void emitTo("main", NAVIGATE_EVENT, { runtime, view }).catch(err => logError("now-playing.open", err)); }} />;
};
