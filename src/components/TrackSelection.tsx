import { useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { TrackMetadata } from "../types/audio";
import { changeSelection, selectedTracks, selectionIdentity } from "../utils/trackSelection";
import { useQueueInsertion } from "./QueueInsertionMenu";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import type { NewPlaylistItem } from "../types/playlist";
import { Check, ListChecks } from "lucide-react";
export function useTrackSelection(context: unknown, tracks: TrackMetadata[], available: boolean) {
  const { t } = useTranslation();
  const identity = selectionIdentity(tracks);
  const visit = useMemo(() => ({}), [context, identity]);
  const live = useRef(visit); live.current = visit;
  const [state, setState] = useState<{ visit: object; indices: Set<number>; anchor: number | null }>({ visit, indices: new Set(), anchor: null });
  const [mode, setMode] = useState<{ visit: object; active: boolean }>({ visit, active: false });
  const [picker, setPicker] = useState<{ visit: object; items: NewPlaylistItem[] } | null>(null);
  const selecting = mode.visit === visit && mode.active;
  const selectable = (index: number) => tracks[index]?.media_locator?.kind !== "radio" && tracks[index]?.media_locator?.kind !== "upnp" && (tracks[index]?.media_locator?.kind !== "navidrome" || !!(tracks[index]?.media_locator as {account_id?:string})?.account_id);
  const indices = state.visit === visit ? state.indices : new Set<number>();
  const queue = useQueueInsertion(visit, available);
  const toggle = (index: number, range: boolean) => setState(previous => {
    const current = previous.visit === visit ? previous : { visit, indices: new Set<number>(), anchor: null };
    return { visit, indices: new Set([...changeSelection(current.indices, index, current.anchor, range)].filter(selectable)), anchor: range && current.anchor !== null ? current.anchor : index };
  });
  const chosen = () => selectedTracks(tracks, indices).map(track => !track.media_locator && track.uri && !track.uri.includes("://") ? { ...track, media_locator: { kind: "local" as const, uri: track.uri } } : track);
  return {
    click(event: MouseEvent, index: number): boolean {
      const nested = (event.target as HTMLElement)?.closest?.("button,input,select,a");
      if (nested && nested !== event.currentTarget) return selecting;
      if (selecting) {
        event.preventDefault(); event.stopPropagation();
        if (selectable(index)) toggle(index, event.shiftKey);
        return true;
      }
      if (!selectable(index) || !(event.ctrlKey || event.metaKey || event.shiftKey)) return false;
      event.preventDefault(); event.stopPropagation(); toggle(index, event.shiftKey); return true;
    },
    selecting,
    checked: (index: number) => indices.has(index),
    checkbox(index: number): ReactNode {
      if (!selecting || !selectable(index)) return null;
      return <input type="checkbox" className="track-selection-check" checked={indices.has(index)} aria-label={t("selection.selectTrack", { title: tracks[index]?.title })}
        onClick={event => { event.stopPropagation(); toggle(index, event.shiftKey); }} onChange={() => {}} onKeyDown={event => event.stopPropagation()} />;
    },
    toolbar: <>{queue.notice}{queue.menu}<div className={`track-selection-toolbar${indices.size > 0 ? " is-active" : ""}`} role="group" aria-label={t("selection.title")}>
      <button type="button" className="track-selection-mode" aria-pressed={selecting} onClick={() => {
        setMode({ visit, active: !selecting });
        if (selecting) setState({ visit, indices: new Set(), anchor: null });
      }}>
        {selecting ? <Check size={14} aria-hidden="true" /> : <ListChecks size={14} aria-hidden="true" />}
        {t(selecting ? "selection.done" : "selection.start")}
      </button>
      {(selecting || indices.size > 0) && <button type="button" onClick={() => setState({ visit, indices: new Set(tracks.map((_, i) => i).filter(selectable)), anchor: 0 })}>{t("selection.all")}</button>}
      {indices.size > 0 && <>
      <strong aria-live="polite">{t("selection.count", { count: indices.size })}</strong>
      {queue.trigger({ track: { title: t("selection.count", { count: indices.size }), artist: "", album: "" }, scope: "selection", load: async () => chosen(), current: () => live.current === visit })}
      <button type="button" onClick={() => { const items = chosen().filter(track => track.media_locator && track.media_locator.kind !== "upnp" && track.media_locator.kind !== "radio" && (track.media_locator.kind !== "navidrome" || !!track.media_locator.account_id)).map(track => ({ media_locator: track.media_locator!, metadata: { title: track.title, artist: track.artist, album: track.album, duration: track.duration ?? undefined } })); if (items.length) setPicker({ visit, items }); }}>{t("selection.playlist")}</button>
      <button type="button" onClick={() => setState({ visit, indices: new Set(), anchor: null })}>{t("selection.clear")}</button>
      </>}
    </div>{picker?.visit === visit && <PlaylistPickerModal items={picker.items} title={t("selection.title")} onClose={() => setPicker(null)} />}</>,
  };
}
