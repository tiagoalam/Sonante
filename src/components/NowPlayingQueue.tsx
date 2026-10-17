import { beginPlaybackIntent, isPlaybackSuperseded } from "../utils/playbackIntent";
import { useLocalQueueArtwork } from "../utils/useLocalQueueArtwork";
import React, { useEffect, useRef, useState } from "react";
import { Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PlaybackStatus, QueueArtworkContext } from "../types/audio";
import { audioService } from "../services/audio";
import { playConfirmedQueuePosition, queueItemArtwork, QueuePreviewReader, queueItemIsCurrent, queueVisibleRange, type QueuePreviewState } from "../utils/nowPlayingPresentation";
import { TrackArtwork } from "./TrackArtwork";

export const NowPlayingQueue: React.FC<{ open: boolean; available: boolean; status?: PlaybackStatus; artworkContext?: QueueArtworkContext; detached?: boolean }> = ({ open, available, status, artworkContext, detached = false }) => {
  const { t } = useTranslation();
  const queueCoherent = status?.playback_error?.code !== "queue_uncertain";
  const queueAvailable = available && queueCoherent;
  const [state, setState] = useState<QueuePreviewState>({ items: [], phase: "idle" });
  const [viewport, setViewport] = useState({ top: 0, height: 500 });
  const [actionBusy, setActionBusy] = useState(false);
  const [actionError, setActionError] = useState(false);
  const actionRef = useRef(false);
  const availableRef = useRef(available);
  availableRef.current = queueAvailable && open;
  const listRef = useRef<HTMLDivElement>(null);
  const readerRef = useRef<QueuePreviewReader | null>(null);
  useEffect(() => {
    availableRef.current = queueAvailable && open;
    const reader = new QueuePreviewReader(audioService.getQueue, setState);
    readerRef.current = reader;
    reader.setActive(open && queueAvailable);
    return () => { availableRef.current = false; reader.setActive(false); readerRef.current = null; };
  }, []);
  useEffect(() => { readerRef.current?.setActive(open && queueAvailable); }, [open, queueAvailable]);
  useEffect(() => {
    const node = listRef.current;
    if (!open || !node) return;
    const update = () => setViewport({ top: node.scrollTop, height: node.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(node);
    return () => observer.disconnect();
  }, [open]);
  const act = async (index?: number) => {
    if (!availableRef.current || actionRef.current) return;
    const intent = beginPlaybackIntent();
    actionRef.current = true; setActionBusy(true); setActionError(false);
    try {
      if (index === undefined) {
        await audioService.clearQueue(intent);
        readerRef.current?.refresh([]);
      } else {
        await playConfirmedQueuePosition(state.items, index, audioService.getQueue, index => audioService.playQueueIndex(index, intent), () => availableRef.current && intent.current());
        readerRef.current?.refresh();
      }
    } catch (error) { if (availableRef.current && !isPlaybackSuperseded(error) && await intent.confirmCurrent()) setActionError(true); }
    finally { actionRef.current = false; setActionBusy(false); }
  };
  const { start, end } = queueVisibleRange(state.items.length, viewport.top, viewport.height);
  const artwork = useLocalQueueArtwork(state.items, start, end, open, queueAvailable, status, artworkContext, detached);
  return <aside data-now-playing-queue-content id="now-playing-queue" hidden={!open} className="np-queue" aria-label={t("queue.title")}>
    <div className="np-queue-heading"><div><h2>{t("queue.title")}</h2><span>{t("common.trackCount", { count: queueCoherent ? state.items.length : 0 })}</span></div>
      {queueCoherent && state.items.length > 0 && <button type="button" onClick={() => void act()} disabled={!queueAvailable || actionBusy} aria-label={t("queue.clear")} title={t("queue.clear")}><Trash2 size={15} /></button>}
    </div>
    {actionError && <p role="alert" className="np-queue-notice">{t("nowPlaying.queueActionFailed")}</p>}
    {!queueCoherent ? <p role="alert" className="np-queue-notice">{t("player.playbackError.queue_uncertain")}</p> : !available ? <p role="status" className="np-queue-notice">{t("player.engineUnavailable")}</p>
      : state.phase === "loading" ? <p role="status" className="np-queue-notice">{t("nowPlaying.queueLoading")}</p>
      : state.phase === "error" ? <p role="alert" className="np-queue-notice">{t("nowPlaying.queueFailed")}</p> : null}
    {available && state.phase === "ready" && !state.items.length && <p className="np-queue-notice">{t("queue.empty")}</p>}
    <div ref={listRef} tabIndex={0} className="np-queue-scroll" aria-label={t("queue.title")}
      onScroll={event => setViewport({ top: event.currentTarget.scrollTop, height: event.currentTarget.clientHeight })}>
      <ol style={{ height: state.items.length * 64, position: "relative" }}>
        {(queueCoherent ? state.items : []).slice(start, end).map((item, offset) => {
          const index = start + offset;
          const current = available && queueItemIsCurrent(item, index, status?.current_media ?? null);
          return <li key={index} aria-current={current ? "true" : undefined} aria-posinset={index + 1} aria-setsize={state.items.length}
            className={`np-queue-row${current ? " np-queue-current" : ""}`} style={{ top: index * 64 }}>
            <button type="button" className="np-queue-track" disabled={!queueAvailable || actionBusy} onClick={() => void act(index)} aria-label={t("nowPlaying.playQueueTrack", { title: item.title || t("player.noTrack") })}>
            <span className="np-queue-number">{index + 1}</span>
            <div className="np-queue-art"><TrackArtwork track={artwork(queueItemArtwork(item, index, status))} /></div>
            <div className="np-queue-label"><p title={item.title}>{item.title || t("player.noTrack")}</p><span title={item.artist}>{item.artist || "—"}</span></div>
            </button>
          </li>;
        })}
      </ol>
    </div>
  </aside>;
};
