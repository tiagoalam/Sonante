import React, { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Compass, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createExploreSession } from "../utils/explore";
import type { ExploreContext, ExploreEntry } from "../utils/explore";
import { localArtworkResolver } from "../utils/localArtworkSession";
import { PlexImage } from "./PlexImage";
import { AlbumArtworkPlaceholder } from "./AlbumArtworkPlaceholder";

export const ExploreAlbumCard: React.FC<{ entry: ExploreEntry; current: () => boolean; revision: number; active: boolean; onOpen: () => void }> = ({ entry, current, revision, active, onOpen }) => {
  const { t } = useTranslation();
  const [cover, setCover] = useState<string | null>(null);
  const live = useRef(current); live.current = current;
  useEffect(() => {
    setCover(null);
    if (entry.source !== "local" || !active || !current()) return;
    let mounted = true;
    const valid = current;
    const cached = localArtworkResolver.get(entry.album.id);
    if (cached) { setCover(cached); return; }
    void localArtworkResolver.resolveLocalOnly(entry.album, () => !mounted || !valid()).then(value => {
      if (mounted && valid()) setCover(value);
    }).catch(() => { /* Artwork failure leaves a placeholder, not a catalog failure. */ });
    return () => { mounted = false; };
  }, [entry, revision, active]);
  return <button type="button" onClick={() => { if (live.current()) onOpen(); }} disabled={!active}
    className="min-w-0 rounded-xl p-2 text-left hover:bg-[#222222] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-60">
    <div className="aspect-square overflow-hidden rounded-lg bg-[#202020]">
      {entry.source === "plex" && entry.album.thumb && active ? <PlexImage image={entry.album.thumb} alt="" className="h-full w-full object-cover" fallback={<AlbumArtworkPlaceholder />} />
        : cover ? <img src={cover} alt="" className="h-full w-full object-cover" /> : <AlbumArtworkPlaceholder />}
    </div>
    <p className="mt-2 truncate text-sm font-semibold text-white" title={entry.album.title}>{entry.album.title || t("explore.untitled")}</p>
    <p className="truncate text-xs text-[#AAAAAA]" title={entry.album.artist}>{entry.album.artist || t("explore.artistNotProvided")}</p>
    <p className="mt-1 text-[11px] text-[#777777]">{t(`explore.sources.${entry.source}`)}</p>
  </button>;
};

interface Props {
  context: ExploreContext;
  active: boolean;
  libraryTitle: string | null;
  ensureLocal: () => void;
  isCurrent: (entry: ExploreEntry) => boolean;
  onOpen: (entry: ExploreEntry) => void;
}
export const ExploreView: React.FC<Props> = ({ context, active, libraryTitle, ensureLocal, isCurrent, onOpen }) => {
  const { t } = useTranslation();
  const session = useMemo(() => createExploreSession(), []);
  session.syncContext(context, active, false);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const ensure = useRef(ensureLocal); ensure.current = ensureLocal;
  useEffect(() => { session.activate(); return () => session.close(); }, [session]);
  useEffect(() => { if (active) ensure.current(); }, [active, context.local.state]);
  const selected = session.visibleEntries();
  return <main className="min-h-0 flex-1 overflow-y-auto bg-[#121212] p-6 md:p-8">
    <h1 className="flex items-center gap-2 text-2xl font-bold text-white"><Compass size={24} />{t("explore.title")}</h1>
    <p className="mt-2 text-sm text-[#999999]">{t("explore.scope", { library: libraryTitle ?? t("explore.noLibrary") })}</p>
    <div className="mt-4 grid gap-2 text-xs sm:grid-cols-2">{(["local", "plex"] as const).map(source => <p key={source}
      role={context[source].state === "unavailable" ? "alert" : "status"} className="rounded-lg border border-[#292929] p-3 text-[#AAAAAA]">
      {t(`explore.sources.${source}`)}: {t(`explore.states.${context[source].state}`)}
    </p>)}</div>
    <div className="mt-7 flex flex-wrap items-center justify-between gap-3">
      <h2 className="text-lg font-bold text-white">{t("explore.shelf")}</h2>
      <button type="button" disabled={!active || session.candidateCount() < 2} onClick={() => session.renew()}
        className="flex items-center gap-2 rounded-lg border border-[#444444] px-3 py-2 text-xs text-white disabled:opacity-40">
        <RefreshCw size={14} />{t("explore.renew")}
      </button>
    </div>
    <p className="mt-2 text-xs text-[#888888]">{t("explore.renewHint")}</p>
    <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">{selected.map(entry => <ExploreAlbumCard key={entry.key} entry={entry}
      revision={state.revision} active={active} current={() => session.canUse(entry, state.revision) && isCurrent(entry)}
      onOpen={() => { if (session.canUse(entry, state.revision) && isCurrent(entry)) onOpen(entry); }} />)}</div>
    {!selected.length && <p role="status" className="mt-6 text-sm text-[#999999]">{t("explore.noSuggestions")}</p>}
  </main>;
};
