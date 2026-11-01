import { sourceEnabled, sourceLocatorEnabled } from "../utils/musicSources";
import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, History, Play, Disc3, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { historyService, type HistoryEntry, type HistoryFile } from "../services/history";
import { audioService } from "../services/audio";
import { localAlbumCatalog } from "../utils/localAlbumCatalogSession";
import { localAlbumForMedia } from "../utils/playerNavigation";
import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { useQueueInsertion } from "./QueueInsertionMenu";
import { LocalBrowserView } from "./LocalBrowserView";
import { NavidromeView } from "./NavidromeView";
import { navidromeService } from "../services/navidrome";
import { AlbumView } from "./AlbumView";
import type { AppConfig } from "../types/config";
import type { PlexAlbum } from "../types/plex";
import type { PlaybackStatus } from "../types/audio";
export function HistoryView({ config, plexIdentity, status, active, isPlaybackAvailable, catalogRefreshRevision }: {
  config: AppConfig; plexIdentity: object; status: PlaybackStatus; active: boolean; isPlaybackAvailable: boolean; catalogRefreshRevision: number;
}) {
  const { t, i18n } = useTranslation();
  const [data, setData] = useState<HistoryFile | null>(null), [error, setError] = useState(false), [busy, setBusy] = useState(false);
  const [source, setSource] = useState<"all" | "local" | "plex" | "navidrome">("all");
  const [confirmClear, setConfirmClear] = useState(false);
  const [detail, setDetail] = useState<{ context: object; local?: string; plex?: PlexAlbum; navidrome?: {account_id:string;id:string} } | null>(null);
  const context = useMemo(() => ({}), [plexIdentity, JSON.stringify(config.local_folders), config.sources, active]);
  const live = useRef<object | null>(context); live.current = context;
  const owner = useRef(false);
  useEffect(() => { live.current = context; return () => { if (live.current === context) live.current = null; }; }, [context]);
  const loadVersion = useRef(0);
  useEffect(() => {
    if (!active) return;
    let current = true; let unlisten: (() => void) | undefined;
    const reload = () => {
      if (!current || owner.current) return;
      const version = ++loadVersion.current;
      void historyService.list().then(value => { if (current && loadVersion.current === version) { setData(value); setError(false); } })
        .catch(() => { if (current && loadVersion.current === version) setError(true); });
    };
    void historyService.subscribe(reload).then(stop => { if (current) { unlisten = stop; reload(); } else stop(); }).catch(() => { if (current) reload(); });
    return () => { current = false; loadVersion.current++; unlisten?.(); };
  }, [active, context]);
  const request = usePlaybackAttempt(context, "player.request");
  const actions = useQueueInsertion(context, active && isPlaybackAvailable);
  const configure = async (enabled?: boolean, clear = false) => {
    if (owner.current) return; owner.current = true; loadVersion.current++; setBusy(true); setError(false);
    try { const value = await historyService.configure(enabled, clear); if (live.current === context) { setData(value); setConfirmClear(false); } }
    catch { if (live.current === context) { setError(true); if (enabled === false) setData(value => value && ({ ...value, enabled: false })); } }
    finally { owner.current = false; if (live.current === context) setBusy(false); }
  };
  const openAlbum = async (entry: HistoryEntry) => {
    if (owner.current || !active || !isPlaybackAvailable) return; owner.current = true; setBusy(true); setError(false);
    try {
      const locator = entry.track.media_locator;
      if (!sourceLocatorEnabled(config, locator)) return;
      if (locator?.kind === "local") {
        const catalog = localAlbumCatalog.getSnapshot();
        const albums = catalog.loaded ? catalog.albums : await audioService.getLocalAlbums();
        const album = localAlbumForMedia({ kind: "local", uri: locator.uri, queue_index: 0 }, albums);
        if (!album) throw "HISTORY_UNAVAILABLE";
        if (live.current === context) setDetail({ context, local: album.id });
      } else if (locator?.kind === "navidrome") {
        const navigation = await navidromeService.songNavigation(locator);
        if (live.current === context) setDetail({context,navidrome:{account_id:navigation.account_id,id:navigation.album_id}});
      } else if (locator?.kind === "plex") {
        const navigation = await historyService.plexNavigation(entry.id);
        if (live.current === context) setDetail({ context, plex: navigation.album });
      }
    } catch { if (live.current === context) setError(true); }
    finally { owner.current = false; if (live.current === context) setBusy(false); }
  };
  const currentDetail = detail?.context === context ? detail : null;
  if (currentDetail) return <div className="flex flex-1 min-w-0 min-h-0 flex-col">
    {currentDetail.local ? <><button type="button" className="px-8 py-3 text-left text-sm" onClick={() => setDetail(null)}><ArrowLeft className="inline mr-2" size={16} />{t("history.title")}</button>
      <LocalBrowserView initialAlbumId={currentDetail.local} onAlbumNavigationBack={() => setDetail(null)} catalogRefreshRevision={catalogRefreshRevision}
        isPlaybackAvailable={isPlaybackAvailable && active} isLibraryUpdating={status.is_updating} onlineArtworkEnabled={config.online_artwork_enabled} configuredLocalFolders={config.local_folders} /></>
      : currentDetail.navidrome ? <NavidromeView available={isPlaybackAvailable && active} embedded initialAlbum={currentDetail.navidrome} onAlbumBack={() => setDetail(null)} />
      : currentDetail.plex && <AlbumView album={currentDetail.plex} active={active} onBack={() => setDetail(null)} isPlaybackAvailable={isPlaybackAvailable} />}
  </div>;
  const visibleSource = source === "all" || sourceEnabled(config, source) ? source : "all";
  const entries = data?.entries.filter(entry => sourceLocatorEnabled(config, entry.track.media_locator) && (visibleSource === "all" || entry.track.media_locator?.kind === visibleSource)) ?? [];
  return <section className="flex min-w-0 flex-1 flex-col overflow-y-auto bg-[var(--theme-surface-base,#121212)]">
    <header className="flex flex-wrap items-center justify-between gap-4 border-b border-[color:var(--theme-border-soft,#333)] p-8">
      <div><h1 className="flex items-center gap-3 text-2xl font-bold"><History size={25} />{t("history.title")}</h1><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#888)]">{t("history.hint")}</p></div>
      <div className="flex flex-wrap items-center gap-4 text-sm"><label className="flex items-center gap-2"><input type="checkbox" checked={data?.enabled ?? false} disabled={!data || busy} onChange={event => void configure(event.target.checked)} />{t("history.record")}</label>
        <button type="button" disabled={!data?.entries.length || busy} onClick={() => setConfirmClear(true)}><Trash2 className="inline mr-2" size={15} />{t("history.clear")}</button></div>
    </header>
    {confirmClear && <div role="alert" className="m-6 rounded-lg border border-[color:var(--theme-border-soft,#444)] p-4 text-sm"><p>{t("history.confirmClear")}</p><div className="mt-3 flex gap-5"><button type="button" disabled={busy} onClick={() => void configure(undefined, true)}>{t("history.clear")}</button><button type="button" disabled={busy} onClick={() => setConfirmClear(false)}>{t("settings.cancel")}</button></div></div>}
    <PlaybackRequestNotice failure={request.failure} />{actions.notice}{actions.menu}
    {error && <p role="alert" className="m-6 text-sm">{t("history.failed")}</p>}
    <nav aria-label={t("history.sources")} className="flex gap-3 px-8 py-5">{(["all", "local", "plex", "navidrome"] as const).filter(item => item === "all" || sourceEnabled(config, item)).map(item => <button key={item} type="button" aria-pressed={visibleSource === item} onClick={() => setSource(item)} className={`rounded-lg border px-5 py-2 text-sm ${visibleSource === item ? "border-[color:var(--theme-border-accent,#e5a00d)] bg-[var(--theme-surface-accent,#332b15)]" : "border-[color:var(--theme-border-soft,#333)]"}`}>{t(`history.${item}`)}</button>)}</nav>
    {!data && !error && <p role="status" className="px-8 text-sm">{t("plex.loading")}</p>}
    {data && !entries.length && <p className="px-8 py-10 text-sm text-[color:var(--theme-fg-muted,#888)]">{t("history.empty")}</p>}
    <ol className="px-8 pb-8">{entries.map(entry => <li key={entry.id} onContextMenu={event => actions.open(event, { track: entry.track })} className="flex items-center gap-4 border-b border-[color:var(--theme-border-soft,#222)] py-3">
      <button type="button" className="shrink-0 rounded-lg p-3 hover:bg-[var(--theme-surface-raised,#252525)]" disabled={!active || !isPlaybackAvailable} aria-label={t("history.play", { title: entry.track.title })} onClick={() => void request.run(intent => audioService.playTracks([entry.track], 0, intent))}><Play size={17} /></button>
      <div className="min-w-0 flex-1"><p className="truncate text-sm font-semibold">{entry.track.title}</p><p className="truncate text-xs text-[color:var(--theme-fg-muted,#888)]">{entry.track.artist} · {entry.track.album}</p>
        <p className="mt-1 text-[11px] text-[color:var(--theme-fg-muted,#888)]">{t(`history.${entry.track.media_locator?.kind ?? "local"}`)} · {new Date(entry.played_at).toLocaleString(i18n.language)}</p></div>
      <button type="button" disabled={busy || !active || !isPlaybackAvailable} onClick={() => void openAlbum(entry)} title={t("history.openAlbum")} aria-label={t("history.openAlbum")}><Disc3 size={18} /></button>{actions.trigger({ track: entry.track })}
    </li>)}</ol>
  </section>;
}
