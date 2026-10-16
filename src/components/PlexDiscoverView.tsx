import React, { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { MixChoiceCard } from "./MixChoiceCard";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ArrowRight, Compass, RefreshCw, Server, Library, Disc3, Shuffle, Play } from "lucide-react";
import type { PlexAlbum, PlexDiscoverHub, PlexDiscoverItem, PlexLibrary, SelectedArtist, PlexMixTrack } from "../types/plex";
import type { NewPlaylistItem } from "../types/playlist";
import { PlexDiscoverSession, type DiscoverScope } from "../utils/plexDiscover";
import { discoverHubTitle, discoverItemKey } from "../utils/plexDiscoverPresentation";
import { PlexImage } from "./PlexImage";
import { AlbumArtworkPlaceholder } from "./AlbumArtworkPlaceholder";

const button = "rounded-lg px-3 py-2 text-xs text-[#BBBBBB] hover:bg-[#282828] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-40";
const DiscoverCard: React.FC<{ item: PlexDiscoverItem; active: boolean; identity: object; showOrigin?: boolean; onPlay?: (item: PlexDiscoverItem) => void; playDisabled?: boolean; onOpen: (target: HTMLButtonElement) => void }> = ({ item, active, identity, showOrigin, onPlay, playDisabled, onOpen }) => {
  const { t } = useTranslation();
  return <div className="group relative w-40 shrink-0 sm:w-44">
    <button type="button" data-discover-item={item.presentation_key ?? discoverItemKey(item)} disabled={!active} onClick={event => onOpen(event.currentTarget)}
    aria-label={item.kind === "track" ? t("discover.openTrackAlbum", { title: item.title, album: item.album?.title }) : t("discover.openItem", { title: item.title })}
    className="group w-40 shrink-0 rounded-xl p-2 text-left hover:bg-[#242424] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] sm:w-44">
    <div className={`aspect-square overflow-hidden bg-[#252525] shadow-lg ${item.kind === "artist" ? "rounded-full" : "rounded-xl"}`}>
      {item.thumb && active ? <PlexImage image={item.thumb} cacheScope={identity} alt="" className="h-full w-full object-cover transition-transform duration-200 group-hover:scale-105" fallback={<AlbumArtworkPlaceholder />} /> : <AlbumArtworkPlaceholder />}
    </div>
    <span className="mt-3 block min-h-10 line-clamp-2 text-sm font-semibold leading-5 text-white" title={item.title}>{item.title}</span>
    <span title={item.subtitle} className="mt-1 block truncate text-xs text-[#999999]">{item.subtitle || t(`discover.kinds.${item.kind}`)}</span>
    {showOrigin && <span title={item.artist_members ? [...new Set(item.artist_members.map(member => member.library_title).filter(Boolean))].join(" · ") : item.library_title} className="mt-1 block truncate text-[10px] text-[#B7A47A]">{item.artist_members ? [...new Set(item.artist_members.map(member => member.library_title).filter(Boolean))].join(" · ") : item.library_title}</span>}
    {item.kind === "track" && <span className="mt-1 block text-[10px] text-[#C6A85B]">{t("discover.openAlbum")}</span>}
  </button>
    {item.kind === "album" && item.album && onPlay && <div className="pointer-events-none absolute inset-x-2 top-2 aspect-square rounded-xl bg-black/40 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 flex items-center justify-center">
      <button type="button" disabled={!active || playDisabled} onClick={event => { event.stopPropagation(); onPlay(item); }}
        aria-label={t("common.playAlbum")} title={t("common.playAlbum")}
        className="pointer-events-auto flex h-12 w-12 items-center justify-center rounded-full bg-[#E5A00D] text-black shadow-lg hover:bg-[#F5B01D] active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white disabled:opacity-50 disabled:cursor-not-allowed">
        <Play size={20} className="ml-1" fill="black" aria-hidden="true" />
      </button>
    </div>}
  </div>;
};
const Shelf: React.FC<{ hub: PlexDiscoverHub; active: boolean; identity: object; session: PlexDiscoverSession; showOrigin?: boolean; onPlay?: (item: PlexDiscoverItem) => void; playDisabled?: boolean; onOpen: (item: PlexDiscoverItem, target: HTMLButtonElement) => void }> = ({ hub, active, identity, session, showOrigin, onPlay, playDisabled, onOpen }) => {
  const { t } = useTranslation(); const track = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ before: false, after: false });
  useLayoutEffect(() => {
    const node = track.current; if (!node) return;
    if (active) node.scrollLeft = session.shelfPosition(hub.id);
    const measure = () => { const next = { before: node.scrollLeft > 1, after: node.scrollLeft + node.clientWidth < node.scrollWidth - 1 };
      setEdges(previous => previous.before === next.before && previous.after === next.after ? previous : next); };
    const observer = new ResizeObserver(measure); observer.observe(node); measure();
    const scrolled = () => { if (active) session.saveShelfPosition(hub.id, node.scrollLeft); measure(); };
    node.addEventListener("scroll", scrolled, { passive: true });
    return () => { observer.disconnect(); node.removeEventListener("scroll", scrolled); };
  }, [hub.id, hub.items, active, session]);
  const title = discoverHubTitle(hub, t);
  return <section aria-label={title} className="min-w-0">
    <div className="mb-2 flex items-center justify-between gap-3"><div className="min-w-0"><h2 className="break-words text-lg font-bold text-white">{title}</h2>{showOrigin && hub.library_title && <p title={hub.library_title} className="mt-1 truncate text-xs text-[#B7A47A]">{hub.library_title}</p>}</div>
      <div className="flex shrink-0 gap-1"><button className={button} disabled={!active || !edges.before} aria-label={t("discover.previous", { title })} onClick={() => track.current?.scrollBy({ left: -(track.current.clientWidth * .8), behavior: "smooth" })}><ArrowLeft size={16} /></button>
      <button className={button} disabled={!active || !edges.after} aria-label={t("discover.next", { title })} onClick={() => track.current?.scrollBy({ left: track.current.clientWidth * .8, behavior: "smooth" })}><ArrowRight size={16} /></button></div>
    </div>
    {hub.dates_missing && <p className="mb-2 text-xs text-[#888888]">{t("discover.datesMissing")}</p>}
    {hub.recent_family && !hub.library_id && !hub.items.length && <p role="status" className="text-xs text-[#888888]">{t("discover.empty")}</p>}
    <div ref={track} tabIndex={0} aria-label={title} className="explore-album-shelf flex gap-3 overflow-x-auto pb-3 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]">
      {hub.items.map(item => <DiscoverCard key={item.presentation_key ?? discoverItemKey(item)} item={item} active={active} identity={identity} showOrigin={showOrigin} onPlay={onPlay} playDisabled={playDisabled} onOpen={target => onOpen(item, target)} />)}
    </div>
  </section>;
};
interface Props {
  session: PlexDiscoverSession; scope: DiscoverScope | null; active: boolean;
  selection: string; libraries: PlexLibrary[]; librariesLoading: boolean; librariesError: boolean;
  onLibraryChange: (id: string) => void; onOpenAlbum: (album: PlexAlbum) => void; onOpenArtist: (artist: SelectedArtist) => void; onLibrary: () => void; onRetryLibraries: () => void;
  onPlayAlbum?: (album: PlexAlbum, current: () => boolean) => Promise<void>;
  onPlayMix?: (tracks: PlexMixTrack[]) => Promise<void>; isPlaybackAvailable?: boolean;
  onSaveMix?: (name: string, items: NewPlaylistItem[]) => Promise<unknown>;
}
export const PlexDiscoverView: React.FC<Props> = ({ session, scope, active, selection, libraries, librariesLoading, librariesError, onLibraryChange, onOpenAlbum, onOpenArtist, onLibrary, onRetryLibraries, onPlayMix, onPlayAlbum, onSaveMix, isPlaybackAvailable = false }) => {
  const { t } = useTranslation(); const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const viewport = useRef<HTMLDivElement>(null); const lastDetail = useRef<string | null>(null);
  const lastAlbum = useRef<{ key: string; target: HTMLButtonElement } | null>(null);
  const lastMix = useRef(false), saveButton = useRef<HTMLButtonElement>(null), saveInput = useRef<HTMLInputElement>(null), wasSaving = useRef(false);
  const live = useRef({ scope, active, isPlaybackAvailable, revision: 0 });
  const revision = live.current.revision + (live.current.scope !== scope || live.current.active !== active || live.current.isPlaybackAvailable !== isPlaybackAvailable ? 1 : 0);
  live.current = { scope, active, isPlaybackAvailable, revision };
  useEffect(() => { live.current.active = active; return () => { live.current.active = false; }; }, []);
  const submitting = useRef(false);
  const [albumPlaying, setAlbumPlaying] = useState(false);
  const [albumPlayError, setAlbumPlayError] = useState(false);
  useEffect(() => { setAlbumPlayError(false); }, [scope]);
  const playAlbum = async (item: PlexDiscoverItem) => {
    const captured = scope;
    const current = () => !!captured && live.current.scope === captured && live.current.revision === revision && live.current.active && live.current.isPlaybackAvailable && session.isScope(captured);
    if (submitting.current || !onPlayAlbum || !current() || !session.owns(item) || item.kind !== "album") return;
    const album = session.album(item); if (!album) return;
    submitting.current = true; setAlbumPlaying(true); setAlbumPlayError(false);
    try { await onPlayAlbum(album, current); }
    catch { if (current()) setAlbumPlayError(true); }
    finally { submitting.current = false; setAlbumPlaying(false); }
  };
  const available = !!scope && session.isScope(scope), all = selection === "all";
  const library = libraries.find(item => item.key === selection);
  useEffect(() => { if (active && available) session.ensure(); }, [session, active, available]);
  useLayoutEffect(() => {
    const node = viewport.current; if (!active || !node) return;
    node.scrollTop = state.styles.opened ? session.stylesPosition : state.decades.opened ? session.decadesPosition : state.detail ? session.detailPosition : state.mix.opened ? session.mixPosition : session.position;
    if (state.styles.opened) {
      lastMix.current = true;
      const target = [...node.querySelectorAll<HTMLElement>("[data-style]")].find(item=>item.dataset.style===state.mixStyle?.key);
      (target ?? node.querySelector<HTMLElement>("h1"))?.focus({preventScroll:true});
    } else if (state.decades.opened) {
      lastMix.current = true;
      const target = state.mixDecade !== null ? node.querySelector<HTMLElement>(`[data-decade="${state.mixDecade}"]`) : null;
      (target ?? node.querySelector<HTMLElement>("h1"))?.focus({ preventScroll: true });
    } else if (state.mix.opened) {
      lastMix.current = true;
      const target = session.mixFocus ? node.querySelector<HTMLElement>(`[data-mix-focus="${session.mixFocus}"]:not(:disabled)`) : null;
      (target ?? node.querySelector<HTMLElement>("h1"))?.focus({ preventScroll: true });
    }
    else if (lastMix.current) { node.querySelector<HTMLElement>(state.mixMode === "style" ? "[data-style-mix]" : state.mixMode === "decade" ? "[data-decade-mix]" : "[data-library-mix]")?.focus({ preventScroll: true }); lastMix.current = false; }
    else if (lastAlbum.current) {
      const saved = lastAlbum.current; const target = saved.target.isConnected ? saved.target : [...node.querySelectorAll<HTMLElement>("[data-discover-item]")].find(item => item.dataset.discoverItem === saved.key);
      target?.focus({ preventScroll: true }); lastAlbum.current = null;
    } else if (state.detail) { lastDetail.current = discoverItemKey(state.detail.item); node.querySelector<HTMLElement>("h1")?.focus({ preventScroll: true }); }
    else if (lastDetail.current) {
      const target = [...node.querySelectorAll<HTMLElement>("[data-discover-item]")].find(item => item.dataset.discoverItem === lastDetail.current);
      target?.focus({ preventScroll: true }); lastDetail.current = null;
    }
  }, [active, state.detail?.item, state.mix.opened, state.decades.opened, state.styles.opened, state.mixMode, scope?.identity, session]);
  useLayoutEffect(() => {
    if (!active || !available) return;
    if (state.mixSave.opened && state.mix.opened) { saveInput.current?.focus(); wasSaving.current = true; }
    else if (wasSaving.current) { if (state.mix.opened) saveButton.current?.focus(); wasSaving.current = false; }
  }, [active, available, state.mixSave.opened, state.mix.opened]);
  const open = (item: PlexDiscoverItem, target: HTMLButtonElement) => {
    if (!active || !available || !session.owns(item)) return;
    const album = session.album(item), artist = session.artist(item);
    if (album || artist) { lastAlbum.current = { key: item.presentation_key ?? discoverItemKey(item), target }; if (album) onOpenAlbum(album); else onOpenArtist(artist!); } else session.open(item);
  };
  const action = (label: string, run: () => void, pending = false, accessibleLabel = label) => <button type="button" className={button} aria-label={accessibleLabel} disabled={!active || pending} onClick={run}><RefreshCw size={14} className="mr-2 inline" />{label}</button>;
  const error = (code: string | null) => <p role="alert" className="text-sm text-[#D8B980]">{t(code === "PLEX_DISCOVER_AUTH" ? "discover.authError" : "discover.error")}</p>;
  const playDisabled = !isPlaybackAvailable || albumPlaying;
  const rows = state.hubs;
  const hero = state.hero;
  const heroHasNotice = hero.phase !== "ready" || !hero.item || hero.partial || hero.selectionPartial || hero.datesMissing || state.libraries.some(row => row.recentPhase === "error");
  const canRenew = hero.phase === "ready" && hero.candidates.some(item => !hero.item || discoverItemKey(item) !== discoverItemKey(hero.item));
  const loading = state.libraries.some(row => row.phase === "loading" || row.phase === "idle");
  const mix = state.mix;
  return <main data-plex-discover className="flex min-h-0 min-w-0 flex-1 flex-col bg-[#121212]">
    <header className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-[#262626] px-6 py-4 sm:px-8">
      <div className="flex items-center gap-2 text-sm font-semibold"><Compass size={18} className="text-[#E5A00D]" />{t("sidebar.plex")} <span className="text-[#555555]">/</span> {t("discover.menu")}</div>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <label className="flex min-w-0 items-center gap-2 text-xs text-[#999999]"><Library size={15} /><span className="sr-only">{t("discover.library")}</span>
          <select aria-label={t("discover.library")} value={selection} disabled={!active || librariesLoading || !libraries.length} onChange={event => onLibraryChange(event.target.value)} className="max-w-64 min-w-0 rounded-lg border border-[#333333] bg-[#202020] px-3 py-2 text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D]">
            {!library && !all && <option value={selection}>{t(selection ? "discover.selectionUnavailable" : "discover.chooseLibrary")}</option>}
            <option value="all">{t("discover.allLibraries")}</option>{libraries.map(item => <option key={item.key} value={item.key}>{item.title}</option>)}
          </select>
        </label>
        {available && !state.detail && !mix.opened && !state.decades.opened && !state.styles.opened && action(t("discover.refreshDiscover"), () => session.retry(), loading)}
      </div>
    </header>
    <div ref={viewport} className="min-h-0 flex-1 overflow-y-auto px-6 py-6 sm:px-8" onScroll={event => { if (active) { if (state.styles.opened) session.stylesPosition = event.currentTarget.scrollTop; else if (state.decades.opened) session.decadesPosition = event.currentTarget.scrollTop; else if (state.detail) session.detailPosition = event.currentTarget.scrollTop; else if (mix.opened) session.mixPosition = event.currentTarget.scrollTop; else session.position = event.currentTarget.scrollTop; } }}>
      {albumPlayError && <p role="alert" className="mb-4 text-sm text-[#E8B0A0]">{t("discover.albumPlayError")}</p>}
      {!available ? <div className="flex min-h-72 flex-col items-center justify-center gap-4 text-center"><Server size={36} className="text-[#E5A00D]" /><p role={librariesError ? "alert" : "status"}>{t(librariesLoading ? "discover.loadingLibraries" : librariesError ? "discover.libraryError" : !libraries.length ? "discover.noLibraries" : library || all ? "discover.contextRequired" : selection ? "discover.selectionUnavailable" : "discover.chooseLibrary")}</p>{librariesError && action(t("discover.retryLibraries"), onRetryLibraries)}<button className={button} onClick={onLibrary}>{t("discover.backLibrary")}</button></div>
      : state.styles.opened ? <div className="space-y-5" onKeyDown={event=>{if(event.key==="Escape" && active){event.stopPropagation();session.closeStyles();}}}>
          <button type="button" className={button} disabled={!active} onClick={()=>session.closeStyles()}><ArrowLeft size={16} className="mr-2 inline" />{t("discover.back")}</button>
          <h1 tabIndex={-1} className="text-3xl font-bold text-white">{t("discover.style.title")}</h1>
          {state.styles.rows.some(row=>row.phase==="idle" || row.phase==="loading") && <p role="status">{t("discover.style.loading")}</p>}
          {state.styles.rows.some(row=>row.partial || row.error) && <p role="status" className="text-sm text-[#C8AE80]">{t("discover.style.partial")}</p>}
          {state.styles.rows.filter(row=>row.error).map(row=><p role="alert" key={row.library.key} className="text-sm text-[#E8B0A0]">{t(row.error==="PLEX_DISCOVER_STYLE_UNSUPPORTED"?"discover.style.unsupported":"discover.mix.libraryError",{library:row.library.title})}</p>)}
          {state.styles.rows.some(row=>row.error) && action(t("discover.retrySelection"),()=>session.retryStyles())}
          <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,190px),1fr))] gap-4">{session.styleOptions().map(style=><MixChoiceCard key={style.key} identity={`style:${style.key}`} kind="style" label={style.title} data-style={style.key}
            disabled={!active || state.styles.rows.some(row=>row.phase==="idle" || row.phase==="loading")} onClick={()=>session.chooseStyle(style.key)} />)}</div>
          {state.styles.rows.every(row=>row.phase==="ready" || row.phase==="error") && !session.styleOptions().length && <p role="status">{t("discover.style.empty")}</p>}
        </div>
      : state.decades.opened ? <div className="space-y-5" onKeyDown={event => { if (event.key === "Escape" && active) { event.stopPropagation(); session.closeDecades(); } }}>
          <button type="button" className={button} disabled={!active} onClick={() => session.closeDecades()}><ArrowLeft size={16} className="mr-2 inline" />{t("discover.back")}</button>
          <h1 tabIndex={-1} className="text-3xl font-bold text-white">{t("discover.decade.title")}</h1>
          {state.decades.rows.some(row => row.phase === "idle" || row.phase === "loading") && <p role="status">{t("discover.decade.loading")}</p>}
          {state.decades.rows.some(row => row.partial || row.error) && <p role="status" className="text-sm text-[#C8AE80]">{t("discover.decade.partial")}</p>}
          {state.decades.rows.filter(row => row.error).map(row => <p key={row.library.key} role="alert" className="text-sm text-[#E8B0A0]">{t("discover.mix.libraryError", { library: row.library.title })}</p>)}
          {state.decades.rows.some(row => row.error) && action(t("discover.retrySelection"), () => session.retryYears())}
          <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,190px),1fr))] gap-4">{session.decadeOptions().map(decade => <MixChoiceCard data-decade={decade} key={decade} identity={`decade:${decade}`} kind="decade"
            label={t("discover.decade.option", { decade })} subtitle={`${decade}–${decade+9}`}
            disabled={!active || state.decades.rows.some(row => row.phase === "idle" || row.phase === "loading")}
            onClick={() => session.chooseDecade(decade)} />)}</div>
          {state.decades.rows.every(row => row.phase === "ready" || row.phase === "error") && !session.decadeOptions().length && <p role="status">{t("discover.decade.empty")}</p>}
        </div>
      : mix.opened ? <div data-library-mix-page className="space-y-5" onKeyDown={event => { if (event.key === "Escape" && active && !state.mixSave.opened) { event.stopPropagation(); session.closeMix(); } }}>
          <button type="button" className={button} disabled={!active || state.mixSave.busy} onClick={() => session.closeMix()}><ArrowLeft size={16} className="mr-2 inline" />{t("discover.back")}</button>
          <h1 tabIndex={-1} className="text-3xl font-bold text-white">{state.mixMode === "style" ? t("discover.style.selection", {style:state.mixStyle?.title}) : state.mixMode === "decade" ? t("discover.decade.selection", { decade: state.mixDecade }) : t("discover.mix.title")}</h1>
          <p className="text-sm text-[#B7A47A]">{all ? t("discover.allLibraries") : library?.title}</p>
            <div className="flex flex-wrap items-center gap-3">
              <p role="status" className="text-sm text-[#D6C8AF]">{t(mix.phase === "loading" ? "discover.mix.loading" : "discover.mix.count", { count: mix.tracks.length })}</p>
              <button data-mix-focus="play" onFocus={() => { session.mixFocus = "play"; }} type="button" className={`${button} bg-[#E5A00D] text-black hover:bg-[#F0B527]`} disabled={!active || !isPlaybackAvailable || !onPlayMix || mix.phase !== "ready" || !mix.tracks.length || mix.playing} onClick={() => { if (active && available && onPlayMix) void session.playMix(onPlayMix); }}>{t(mix.playing ? "discover.mix.playing" : "discover.mix.play")}</button>
              <button data-mix-focus="save" onFocus={() => { session.mixFocus = "save"; }} ref={saveButton} type="button" className={button} disabled={!active || !onSaveMix || mix.phase !== "ready" || !mix.tracks.length || state.mixSave.busy} onClick={() => session.openMixSave()}>{t("discover.mix.save")}</button>
              <button data-mix-focus="another" onFocus={() => { session.mixFocus = "another"; }} type="button" className={button} disabled={!active || mix.phase === "loading" || mix.playing || state.mixSave.busy} onClick={() => { session.cancelMixSave(); session.generateMix(); }}>{t("discover.mix.another")}</button>
            </div>
            {state.mixMode === "decade" && <button data-mix-focus="change" onFocus={() => { session.mixFocus = "change"; }} type="button" className={button} disabled={!active || state.mixSave.busy || mix.playing} onClick={() => session.openDecades()}>{t("discover.decade.change")}</button>}
            {state.mixMode === "style" && <button data-mix-focus="change" onFocus={()=>{session.mixFocus="change";}} type="button" className={button} disabled={!active || state.mixSave.busy || mix.playing} onClick={()=>session.openStyles()}>{t("discover.style.change")}</button>}
            {state.mixSave.saved && <p role="status" className="text-sm text-[#A7CE9C]">{t("discover.mix.saved")}</p>}
            {mix.playError && <p role="alert" className="text-sm text-[#E8B0A0]">{t("discover.mix.playError")}</p>}
            {mix.phase === "error" && <p role="alert" className="text-sm text-[#E8B0A0]">{t("discover.mix.error")}</p>}
            {!!mix.tracks.length && mix.partial && <p role="status" className="text-xs text-[#C8AE80]">{t("discover.mix.partial")}</p>}
            {mix.limited && <p className="text-xs text-[#A69F91]">{t("discover.mix.limited")}</p>}
            {mix.issues.map((issue, index) => <p key={index} role="alert" className="text-xs text-[#E8B0A0]">{t(issue.error === "PLEX_DISCOVER_AUTH" ? "discover.mix.libraryAuth" : "discover.mix.libraryError", { library: issue.library })}</p>)}
            {mix.phase === "ready" && !mix.tracks.length && <p role="status" className="text-sm text-[#A69F91]">{t("discover.mix.empty")}</p>}
            {!!mix.tracks.length && <div className="overflow-x-auto rounded-xl border border-white/5 bg-black/15">
              <table className="w-full text-left text-sm"><caption className="sr-only">{t("discover.mix.preview")}</caption><thead className="sticky top-0 bg-[#25221C] text-xs text-[#A69F91]"><tr><th className="px-3 py-2" scope="col">{t("discover.mix.track")}</th><th className="px-3 py-2" scope="col">{t("discover.mix.artist")}</th><th className="px-3 py-2" scope="col">{t("discover.mix.album")}</th>{all && <th className="px-3 py-2" scope="col">{t("discover.mix.origin")}</th>}</tr></thead>
                <tbody className="divide-y divide-white/5">{mix.tracks.map(track => <tr key={JSON.stringify([track.server_id, track.library_id, track.rating_key])}><td className="px-3 py-2 text-[#DDD4C4]">{track.title}</td><td className="px-3 py-2 text-[#AAA397]">{track.artist || t("discover.mix.unknown")}</td><td className="px-3 py-2 text-[#AAA397]">{track.album_title || t("discover.mix.unknown")}</td>{all && <td className="px-3 py-2 text-xs text-[#C4AD7D]">{track.library_title}</td>}</tr>)}</tbody>
              </table>
            </div>}
        </div>
      : state.detail ? <div className="space-y-6"><button className={button} onClick={() => session.back()} disabled={!active}><ArrowLeft size={16} className="mr-2 inline" />{t("discover.back")}</button>
        <div><p className="text-xs uppercase tracking-widest text-[#B99F66]">{t(`discover.kinds.${state.detail.item.kind}`)}</p><h1 tabIndex={-1} className="mt-2 break-words text-3xl font-bold text-white">{state.detail.item.title}</h1><p className="mt-2 text-xs text-[#B7A47A]">{state.detail.item.library_title}</p></div>
        {state.detail.phase === "loading" && <p role="status">{t("discover.loadingSelection")}</p>}
        {state.detail.phase === "error" && <div className="space-y-2">{error(state.detail.error)}{action(t("discover.retrySelection"), () => session.retryDetail())}</div>}
        {state.detail.hub && <><Shelf session={session} hub={state.detail.hub} active={active} identity={scope!.identity as object} onPlay={onPlayAlbum ? playAlbum : undefined} playDisabled={playDisabled} onOpen={open} />{!state.detail.hub.items.length && <p role="status">{t("discover.emptySelection")}</p>}{state.detail.hub.partial && <p className="text-xs text-[#777777]">{t("discover.finiteSelection")}</p>}</>}
      </div> : <div className="space-y-8">
        <div className="relative flex min-h-56 flex-col items-start gap-6 sm:flex-row sm:items-center overflow-hidden rounded-2xl border border-[#514023]/50 bg-gradient-to-br from-[#3A2C16] via-[#24201A] to-[#181818] p-6 sm:p-8">
          <div className="relative min-w-0 flex-1"><p className="text-[10px] font-semibold uppercase tracking-[.22em] text-[#D0AF70]">{t("discover.fromServer")}</p><h1 className="mt-3 text-3xl font-black tracking-tight text-white sm:text-4xl">{t("discover.title")}</h1><p className="mt-3 max-w-lg text-sm leading-relaxed text-[#B8B0A3]">{all ? t("discover.allDescription") : t("discover.description", { library: library?.title })}</p>
            <button className={`${button} mt-4 inline-flex items-center gap-2 bg-black/20`} disabled={!active} onClick={onLibrary}><Disc3 size={15} />{t("discover.backLibrary")}</button>{hero.item && <button type="button" className={`${button} ml-2 mt-4 inline-flex items-center gap-2`} disabled={!active || !canRenew} onClick={() => session.anotherDiscovery()}><Shuffle size={14} />{t("discover.another")}</button>}
            {heroHasNotice && <div className="mt-3 space-y-1 text-xs text-[#A49B8B]" aria-live="polite">
              {hero.phase === "idle" || hero.phase === "loading" ? <p role="status">{t("discover.heroLoading")}</p> : hero.phase === "error" ? <p role="alert">{t("discover.heroUnavailable")}</p> : !hero.item ? <p role="status">{t("discover.heroEmpty")}</p> : null}
              {(hero.partial || hero.selectionPartial) && <p>{t("discover.heroPartial")}</p>}{hero.datesMissing && <p>{t("discover.heroDatesMissing")}</p>}
              {state.libraries.filter(row => row.recentPhase === "error").map(row => <div key={row.library.key} className="flex flex-wrap items-center gap-2"><span>{row.library.title}</span>{error(row.recentError)}{action(t("discover.retryHero"), () => session.retryHero(row.library.key), false, t("discover.retryHeroFor", { library: row.library.title }))}</div>)}
            </div>}</div>
          {hero.item && <div className="shrink-0 sm:rotate-3"><DiscoverCard item={hero.item} active={active} identity={scope!.identity as object} showOrigin={all} onPlay={onPlayAlbum ? playAlbum : undefined} playDisabled={playDisabled} onOpen={target => open(hero.item!, target)} /></div>}
        </div>
        <section aria-label={t("discover.mixes")} className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,200px),1fr))] gap-4">
          <button type="button" data-library-mix aria-label={t("discover.mix.title")} disabled={!active || mix.playing} onClick={() => session.openMix("library")}
            style={{ backgroundColor: "#76513A", backgroundImage: "linear-gradient(135deg, rgba(255,255,255,0.09), transparent 70%)" }}
            className="relative flex h-32 min-w-0 w-full items-end overflow-hidden rounded-2xl p-5 text-left font-bold text-white transition hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-50">
            <Shuffle aria-hidden="true" size={72} className="absolute -right-2 -top-2 rotate-12 text-white/20" /><span className="relative text-xl">{t("discover.mix.title")}</span>
          </button>
          <button type="button" data-decade-mix aria-label={t("discover.decade.title")} disabled={!active || mix.playing || state.mixSave.busy} onClick={() => session.openDecades()}
            style={{ backgroundColor: "#455777", backgroundImage: "linear-gradient(135deg, rgba(255,255,255,0.09), transparent 70%)" }}
            className="relative flex h-32 min-w-0 w-full items-end overflow-hidden rounded-2xl p-5 text-left font-bold text-white transition hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-50">
            <Disc3 aria-hidden="true" size={72} className="absolute -right-2 -top-2 rotate-12 text-white/20" /><span className="relative text-xl">{t("discover.decade.title")}</span>
          </button>
          <button type="button" data-style-mix aria-label={t("discover.style.title")} disabled={!active || mix.playing || state.mixSave.busy} onClick={()=>session.openStyles()}
            style={{backgroundColor:"#385C50",backgroundImage:"linear-gradient(135deg, rgba(255,255,255,0.09), transparent 70%)"}}
            className="relative flex h-32 min-w-0 w-full items-end overflow-hidden rounded-2xl p-5 text-left font-bold text-white transition hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-50">
            <Disc3 aria-hidden="true" size={72} className="absolute -right-2 -top-2 rotate-12 text-white/20" /><span className="relative text-xl">{t("discover.style.title")}</span>
          </button>
        </section>
        {all && loading && <p role="status" className="text-xs text-[#AAAAAA]">{t("discover.libraryProgress", { done: state.libraries.filter(row => row.phase === "ready" || row.phase === "error").length, total: state.totalLibraries })}</p>}
        {all && <p className="text-xs text-[#888888]">{t("discover.allLimits")}</p>}
        {state.libraries.filter(row => (!all && row.phase === "loading") || row.phase === "error" || row.phase === "ready" && row.foundHubs === 0).map(row => <div key={row.library.key} className="space-y-2 rounded-xl border border-[#303030] px-4 py-3">
          {all && <h2 className="break-words text-sm font-semibold">{row.library.title}</h2>}
          {row.phase === "loading" ? <p role="status" className="text-sm text-[#AAAAAA]">{t("discover.loading")}</p> : row.phase === "error" ? <>{error(row.error)}{action(t("discover.retryRecommendations"), () => session.retry(row.library.key), false, t("discover.retryRecommendationsFor", { library: row.library.title }))}</> : <p role="status" className="text-sm text-[#AAAAAA]">{t("discover.empty")}</p>}
        </div>)}
        {rows.map(hub => <Shelf session={session} key={hub.id} hub={hub} active={active} identity={scope!.identity as object} showOrigin={all} onPlay={onPlayAlbum ? playAlbum : undefined} playDisabled={playDisabled} onOpen={open} />)}
      </div>}
    </div>
    {active && available && mix.opened && state.mixSave.opened && <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm" role="dialog" aria-modal="true" aria-label={t("discover.mix.save")}
      onMouseDown={event => { if (event.target === event.currentTarget && !state.mixSave.busy) session.cancelMixSave(); }}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!state.mixSave.busy) session.cancelMixSave(); }
        if (event.key === "Tab") {
          const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled)')];
          if (!controls.length) { event.preventDefault(); return; }
          const first = controls[0], last = controls[controls.length - 1], focused = event.currentTarget.ownerDocument.activeElement;
          if (event.shiftKey && focused === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && focused === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <form className="w-full max-w-md space-y-4 rounded-2xl border border-[#333333] bg-[#171717] p-5 shadow-2xl" onSubmit={event => { event.preventDefault(); if (onSaveMix) void session.saveMix(onSaveMix); }}>
        <h2 className="font-bold text-white">{t("discover.mix.save")}</h2>
        <label className="block text-sm text-[#AAAAAA]">{t("playlists.namePlaceholder")}
          <input ref={saveInput} autoFocus aria-invalid={!!state.mixSave.error} aria-describedby={state.mixSave.error ? "mix-save-error" : undefined} value={state.mixSave.name} disabled={state.mixSave.busy} onChange={event => session.setMixSaveName(event.target.value)} className="mt-2 w-full rounded-lg border border-[#3A3A3A] bg-[#111111] px-3 py-2 text-white outline-none focus:border-[#E5A00D]" />
        </label>
        {state.mixSave.error && <p id="mix-save-error" role="alert" className="text-sm text-red-200">{t(state.mixSave.error)}</p>}
        <div className="flex justify-end gap-2"><button type="button" className={button} disabled={state.mixSave.busy} onClick={() => session.cancelMixSave()}>{t("playlists.cancel")}</button><button type="submit" className="rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black disabled:opacity-50" disabled={state.mixSave.busy}>{t(state.mixSave.busy ? "discover.mix.saving" : "discover.mix.saveConfirm")}</button></div>
      </form>
    </div>}
  </main>;
};
