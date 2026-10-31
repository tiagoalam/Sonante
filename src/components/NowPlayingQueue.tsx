import { useQueueInsertion } from "./QueueInsertionMenu";
import { loadQueuedAlbum } from "../utils/queueAlbumLoader";
import { beginPlaybackIntent, isPlaybackSuperseded } from "../utils/playbackIntent";
import { useLocalQueueArtwork } from "../utils/useLocalQueueArtwork";
import React, { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Trash2, Search, X, LocateFixed } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PlaybackStatus, QueueArtworkContext, TrackMetadata } from "../types/audio";
import { audioService } from "../services/audio";
import { playConfirmedQueuePosition, queueItemArtwork, QueuePreviewReader, queueItemIsCurrent, queueVisibleRange, type QueuePreviewState } from "../utils/nowPlayingPresentation";
import { confirmedQueueIndex, formatQueueTime, queuePresentation, type QueueTime } from "../utils/queuePresentation";
import { TrackArtwork } from "./TrackArtwork";

export const NowPlayingQueue: React.FC<{ open: boolean; available: boolean; status?: PlaybackStatus; artworkContext?: QueueArtworkContext; detached?: boolean }> = ({ open, available, status, artworkContext, detached = false }) => {
  const { t } = useTranslation();
  const queueCoherent = status?.playback_error?.code !== "queue_uncertain";
  const queueAvailable = available && queueCoherent;
  const [state, setState] = useState<QueuePreviewState>({ items: [], phase: "idle" });
  const [viewport, setViewport] = useState({ top: 0, height: 500 });
  const [actionBusy, setActionBusy] = useState(false);
  const [actionError, setActionError] = useState(false);
  const [filter, setFilter] = useState("");
  const [jump, setJump] = useState<{ index: number } | null>(null);
  const filterId = useId();
  const filterRef = useRef<HTMLInputElement>(null);
  const presentation = useMemo(() => queuePresentation(state.items), [state.items]);
  const entries = useMemo(() => queueCoherent ? presentation.filter(filter) : [], [presentation, filter, queueCoherent]);
  const ready = queueAvailable && state.phase === "ready";
  const queueContext = useMemo(() => ({}), [presentation.identity, open, queueAvailable]);
  const queueActions = useQueueInsertion(queueContext, open && ready);
  const queueRequest = (track: TrackMetadata, index: number) => ({ track: track.media_locator ? track : { ...track, media_locator: { kind: "local" as const, uri: track.uri || "" } },
    album: () => loadQueuedAlbum(track, index) });
  const currentIndex = ready && status?.snapshot_consistent !== false ? confirmedQueueIndex(state.items, status?.current_media) : null;
  const remaining = presentation.remaining(status, ready);
  const timeLabel = (value: QueueTime | null) => !value ? "—" : value.unknown
    ? value.seconds > 0 ? t("queue.timeAtLeast", { time: formatQueueTime(value.seconds) }) : t("queue.timeUnknown") : formatQueueTime(value.seconds);
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
  useLayoutEffect(() => {
    if (!open || !jump || !listRef.current) return;
    const position = entries.findIndex(entry => entry.index === jump.index);
    if (position >= 0) {
      const node = listRef.current;
      node.scrollTop = Math.max(0, position * 64 - (node.clientHeight - 64) / 2);
      setViewport({ top: node.scrollTop, height: node.clientHeight });
    }
    setJump(null);
  }, [open, jump, entries]);
  const updateFilter = (value: string) => {
    setFilter(value); setJump(null);
    if (listRef.current) listRef.current.scrollTop = 0;
    setViewport(value => ({ ...value, top: 0 }));
  };
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
  const { start, end } = queueVisibleRange(entries.length, viewport.top, viewport.height);
  const visibleEntries = entries.slice(start, end);
  const artwork = useLocalQueueArtwork(state.items, start, end, open, queueAvailable, status, artworkContext, detached, visibleEntries.map(entry => entry.index));
  return <aside data-now-playing-queue-content id="now-playing-queue" hidden={!open} className="np-queue" aria-label={t("queue.title")}>
    {queueActions.notice}{queueActions.menu}
    <div className="np-queue-heading"><div><h2>{t("queue.title")}</h2><span aria-live="polite" aria-atomic="true">{filter.trim() && queueCoherent
      ? t("queue.filteredCount", { count: entries.length, total: state.items.length }) : t("common.trackCount", { count: queueCoherent ? state.items.length : 0 })}</span></div>
      {queueCoherent && state.items.length > 0 && <button type="button" onClick={() => void act()} disabled={!queueAvailable || actionBusy}
        aria-label={t(filter.trim() ? "queue.clearAll" : "queue.clear")} title={t(filter.trim() ? "queue.clearAll" : "queue.clear")}><Trash2 size={15} /></button>}
    </div>
    <div className="np-queue-tools">
      <label className="np-queue-filter" htmlFor={filterId}><Search size={14} aria-hidden="true" />
        <input ref={filterRef} id={filterId} type="search" value={filter} maxLength={160} aria-label={t("queue.filterLabel")} placeholder={t("queue.filterPlaceholder")}
          onChange={event => updateFilter(event.target.value)} onKeyDown={event => {
            if (event.key === "Escape" && filter && !event.nativeEvent.isComposing) { event.preventDefault(); event.stopPropagation(); updateFilter(""); }
          }} />
        {filter && <button type="button" aria-label={t("queue.clearFilter")} title={t("queue.clearFilter")} onClick={() => { updateFilter(""); filterRef.current?.focus(); }}><X size={14} /></button>}
      </label>
      <button type="button" className="np-queue-jump" disabled={currentIndex === null} onClick={() => {
        if (currentIndex !== null) { setFilter(""); setJump({ index: currentIndex }); }
      }}><LocateFixed size={14} aria-hidden="true" />{t("queue.jumpToCurrent")}</button>
      {ready && state.items.length > 0 && <div className="np-queue-times">
        <span>{t("queue.totalTime", { time: timeLabel(presentation.total) })}</span>
        <span title={t("queue.remainingHint")}>{t("queue.remainingTime", { time: timeLabel(remaining) })}</span>
      </div>}
    </div>
    {actionError && <p role="alert" className="np-queue-notice">{t("nowPlaying.queueActionFailed")}</p>}
    {!queueCoherent ? <p role="alert" className="np-queue-notice">{t("player.playbackError.queue_uncertain")}</p> : !available ? <p role="status" className="np-queue-notice">{t("player.engineUnavailable")}</p>
      : state.phase === "loading" ? <p role="status" className="np-queue-notice">{t("nowPlaying.queueLoading")}</p>
      : state.phase === "error" ? <p role="alert" className="np-queue-notice">{t("nowPlaying.queueFailed")}</p> : null}
    {available && state.phase === "ready" && !state.items.length && <p className="np-queue-notice">{t("queue.empty")}</p>}
    {queueCoherent && state.items.length > 0 && !entries.length && <p role="status" className="np-queue-notice">{t("queue.noMatches")}</p>}
    <div ref={listRef} tabIndex={0} className="np-queue-scroll" aria-label={t("queue.title")}
      onScroll={event => setViewport({ top: event.currentTarget.scrollTop, height: event.currentTarget.clientHeight })}>
      <ol style={{ height: entries.length * 64, position: "relative" }}>
        {visibleEntries.map(({ item, index }, offset) => {
          const displayIndex = start + offset;
          const current = available && queueItemIsCurrent(item, index, status?.current_media ?? null);
          return <li key={index} data-queue-index={index} onContextMenu={item.media_locator?.kind === "upnp" ? undefined : event => queueActions.open(event, queueRequest(item, index))} aria-current={current ? "true" : undefined} aria-posinset={displayIndex + 1} aria-setsize={entries.length}
            className={`np-queue-row${current ? " np-queue-current" : ""}`} style={{ top: displayIndex * 64 }}>
            <button type="button" className="np-queue-track" disabled={!queueAvailable || actionBusy} onClick={() => void act(index)} aria-label={t("nowPlaying.playQueueTrack", { title: item.title || t("player.noTrack") })}>
            <span className="np-queue-number">{index + 1}</span>
            <div className="np-queue-art"><TrackArtwork track={artwork(queueItemArtwork(item, index, status))} /></div>
            <div className="np-queue-label"><p title={item.title}>{item.title || t("player.noTrack")}</p><span title={[item.artist, item.album].filter(Boolean).join(" · ")}>{[item.artist, item.album].filter(Boolean).join(" · ") || "—"}</span></div>
            <span className="np-queue-duration">{Number.isFinite(item.duration) && item.duration! > 0 ? formatQueueTime(item.duration!) : "—"}</span>
            </button>{item.media_locator?.kind !== "upnp" && queueActions.trigger(queueRequest(item, index))}
          </li>;
        })}
      </ol>
    </div>
  </aside>;
};
