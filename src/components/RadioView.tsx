import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Radio, Search, Heart, Play, Square, Plus, Globe2, LoaderCircle, X, Pencil, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { radioService } from "../services/radio";
import { RadioDirectorySession, RADIO_PAGE_SIZE, radioStationMatches } from "../utils/radioDirectory";
import { beginPlaybackIntent, isPlaybackSuperseded } from "../utils/playbackIntent";
import type { PlaybackIntent } from "../utils/playbackIntent";
import type { PlaybackStatus } from "../types/audio";
import type { RadioFilters, RadioQuery, RadioStation } from "../types/radio";
function StationLogo({ station }: { station: RadioStation }) {
  const [failed, setFailed] = useState<string | null>(null);
  return <div className="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-raised,#252525)] text-[color:var(--theme-fg-accent,#e5a00d)]">
    {station.favicon && station.favicon !== failed ? <img src={station.favicon} alt="" loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(station.favicon)} className="h-full w-full object-contain p-1" /> : <Radio size={28} />}
  </div>;
}
export function RadioView({ active, available, status }: { active: boolean; available: boolean; status: PlaybackStatus }) {
  const { t, i18n } = useTranslation();
  const [tab, setTab] = useState<"directory" | "favorites">("directory");
  const [draft, setDraft] = useState({ name: "", country: "", genre: "" });
  const [query, setQuery] = useState<RadioQuery>({ ...draft, offset: 0 });
  const [filtersFailed, setFiltersFailed] = useState(false), [filterRetry, setFilterRetry] = useState(0);
  const [filters, setFilters] = useState<RadioFilters>({ countries: [], genres: [] });
  const [favorites, setFavorites] = useState<RadioStation[]>([]), [favoriteError, setFavoriteError] = useState(false);
  const [favoriteBusy, setFavoriteBusy] = useState(false), [favoritesLoaded, setFavoritesLoaded] = useState(false);
  const [manualOpen, setManualOpen] = useState(false), [manual, setManual] = useState({ name: "", url: "" });
  const [manualId, setManualId] = useState<string | null>(null);
  const [playState, setPlayState] = useState<{ id: string; busy: boolean; failed: boolean } | null>(null);
  const session = useMemo(() => new RadioDirectorySession(radioService.search), []);
  const directory = useSyncExternalStore(session.subscribe, session.getSnapshot);
  const visit = useMemo(() => ({}), [active]);
  const live = useRef<object | null>(visit); live.current = active ? visit : null;
  const manualDialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const node = manualDialog.current; if (!manualOpen || !node) return; const origin = document.activeElement as HTMLElement | null; node.showModal(); return () => { node.close(); origin?.focus?.(); }; }, [manualOpen]);
  const playOwner = useRef<PlaybackIntent | null>(null), saving = useRef(false), favoriteVersion = useRef(0);
  useEffect(() => { live.current = active ? visit : null; if (!active) { setFavoriteBusy(false); setPlayState(null); setManualOpen(false); } return () => { live.current = null; playOwner.current?.cancel(); playOwner.current = null; }; }, [active, visit]);
  useEffect(() => { session.set(query, active && tab === "directory"); return () => session.close(); }, [session, query, active, tab]);
  useEffect(() => {
    if (!active) return;
    let current = true; const version = ++favoriteVersion.current;
    void radioService.favorites().then(value => { if (current && version === favoriteVersion.current) { setFavorites(value); setFavoritesLoaded(true); setFavoriteError(false); } })
      .catch(() => { if (current && version === favoriteVersion.current) { setFavoriteError(true); setFavoritesLoaded(true); } });
    return () => { current = false; };
  }, [active]);
  useEffect(() => {
    if (!active) return;
    let current = true; setFiltersFailed(false);
    void radioService.filters().then(value => { if (current) setFilters(value); }).catch(() => { if (current) setFiltersFailed(true); });
    return () => { current = false; };
  }, [active, filterRetry]);
  const countryName = (code: string) => {
    if (!code) return t("radio.unknownCountry");
    try { return new Intl.DisplayNames([i18n.language], { type: "region" }).of(code) || code; } catch { return code; }
  };
  const sortedCountries = useMemo(() => {
    const countries = new Map(filters.countries.map(choice => [choice.name, choice]));
    for (const station of favorites) if (/^[A-Z]{2}$/.test(station.country_code) && !countries.has(station.country_code)) countries.set(station.country_code, { name: station.country_code, stationcount: 0 });
    return [...countries.values()].sort((a, b) => countryName(a.name).localeCompare(countryName(b.name), i18n.language));
  }, [filters.countries, favorites, i18n.language]);
  const openManual = (station?: RadioStation) => {
    setManualId(station?.id ?? null);
    setManual({ name: station?.name ?? "", url: station?.stream_url ?? "" });
    setFavoriteError(false);
    setManualOpen(true);
  };
  const clearFilters = () => {
    setDraft({ name: "", country: "", genre: "" });
    setQuery({ name: "", country: "", genre: "", offset: 0 });
  };
  const save = async (operation: () => Promise<RadioStation[]>, manualAction = false) => {
    if (live.current !== visit || saving.current) return;
    saving.current = true; favoriteVersion.current++; setFavoriteBusy(true); setFavoriteError(false);
    try { const value = await operation(); if (live.current === visit) { setFavorites(value); setFavoritesLoaded(true); if (manualAction) { setManualOpen(false); setManual({ name: "", url: "" }); clearFilters(); setTab("favorites"); } } }
    catch { if (live.current === visit) setFavoriteError(true); }
    finally { saving.current = false; if (live.current === visit) setFavoriteBusy(false); }
  };
  const play = async (station: RadioStation) => {
    if (!active || !available || playOwner.current) return;
    const intent = beginPlaybackIntent(); playOwner.current = intent;
    setPlayState({ id: station.id, busy: true, failed: false });
    try { await radioService.play(station.id, intent); if (live.current === visit && intent.current() && await intent.confirmCurrent()) setPlayState(null); }
    catch (error) { if (live.current === visit && !isPlaybackSuperseded(error) && await intent.confirmCurrent()) setPlayState({ id: station.id, busy: false, failed: true }); }
    finally { if (playOwner.current === intent) playOwner.current = null; }
  };
  const stop = async () => {
    if (!active || !available || status.current_media?.kind !== "radio" || playOwner.current) return;
    const intent = beginPlaybackIntent(); playOwner.current = intent;
    setPlayState({ id: status.current_media.station_id, busy: true, failed: false });
    try { await radioService.stop(status.current_media.station_id, intent); if (live.current === visit) setPlayState(null); }
    catch (error) { if (live.current === visit && !isPlaybackSuperseded(error) && await intent.confirmCurrent()) setPlayState({ id: status.current_media.station_id, busy: false, failed: true }); }
    finally { if (playOwner.current === intent) playOwner.current = null; }

  };
  const stations = tab === "favorites" ? favorites.filter(station => radioStationMatches(station, { ...draft, offset: 0 })) : directory.stations;
  const favoriteIds = new Set(favorites.map(station => station.id));
  const loading = tab === "directory" && directory.phase === "loading";
  return <section data-radio-view className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]">
    <header className="shrink-0 border-b border-[color:var(--theme-border-soft,#292929)] px-6 py-6 sm:px-8">
      <div className="flex flex-wrap items-start justify-between gap-4"><div><h1 className="flex items-center gap-3 text-2xl font-bold"><Radio size={26} className="text-[color:var(--theme-fg-accent,#e5a00d)]" />{t("radio.title")}</h1><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.subtitle")}</p></div>
        <button type="button" onClick={() => openManual()} className="flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#3a3a3a)] px-4 py-2 text-xs hover:bg-[var(--theme-surface-raised,#252525)]"><Plus size={15} />{t("radio.addManual")}</button></div>
      <nav aria-label={t("radio.navigation")} className="mt-6 flex gap-2">{(["directory", "favorites"] as const).map(item => <button key={item} type="button" aria-pressed={tab === item} onClick={() => setTab(item)} className={`flex items-center gap-2 rounded-lg border px-5 py-2.5 text-sm ${tab === item ? "border-[color:var(--theme-border-accent,#e5a00d)] bg-[var(--theme-surface-accent,#332b15)] text-[color:var(--theme-fg-accent,#e5a00d)]" : "border-[color:var(--theme-border-soft,#333)] text-[color:var(--theme-fg-secondary,#aaa)]"}`}>{item === "directory" ? <Globe2 size={17} /> : <Heart size={17} />}{t(`radio.${item}`)}{item === "favorites" && <span className="text-xs opacity-70">({favorites.length})</span>}</button>)}</nav>
      <form className="mt-5 flex flex-wrap items-end gap-3" onSubmit={event => { event.preventDefault(); setQuery({ ...draft, offset: 0 }); }}>
        <label className="min-w-48 flex-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.name")}<input type="search" maxLength={120} value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} placeholder={t("radio.searchPlaceholder")} className="mt-1.5 block w-full rounded-lg border border-[color:var(--theme-border-soft,#363636)] bg-[var(--theme-surface-panel,#191919)] px-3 py-2.5 text-sm text-[color:var(--theme-fg-primary,#eee)]" /></label>
        <label className="w-44 text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.country")}<select value={draft.country} onChange={event => setDraft({ ...draft, country: event.target.value })} className="mt-1.5 block w-full rounded-lg border border-[color:var(--theme-border-soft,#363636)] bg-[var(--theme-surface-panel,#191919)] px-3 py-2.5 text-sm text-[color:var(--theme-fg-primary,#eee)]"><option value="">{t("radio.allCountries")}</option>{sortedCountries.map(country => <option key={country.name} value={country.name}>{countryName(country.name)}</option>)}</select></label>
        <label className="w-44 text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.genre")}<input list="radio-genres" maxLength={80} value={draft.genre} onChange={event => setDraft({ ...draft, genre: event.target.value })} placeholder={t("radio.allGenres")} className="mt-1.5 block w-full rounded-lg border border-[color:var(--theme-border-soft,#363636)] bg-[var(--theme-surface-panel,#191919)] px-3 py-2.5 text-sm text-[color:var(--theme-fg-primary,#eee)]" /><datalist id="radio-genres">{filters.genres.map(genre => <option key={genre.name} value={genre.name} />)}</datalist></label>
        <button type="submit" className="flex items-center gap-2 rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-5 py-2.5 text-sm font-semibold text-black"><Search size={16} />{t("radio.search")}</button>
        <button type="button" onClick={clearFilters} className="flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#3a3a3a)] px-4 py-2.5 text-sm text-[color:var(--theme-fg-secondary,#aaa)] hover:bg-[var(--theme-surface-raised,#252525)]"><X size={15} />{t("radio.clearFilters")}</button>
      </form>
      {filtersFailed && <button type="button" onClick={() => setFilterRetry(value => value + 1)} className="mt-3 text-xs text-[color:var(--theme-fg-muted,#999)] underline">{t("radio.filtersError")}</button>}
    </header>
    <div className="min-h-0 flex-1 overflow-y-auto px-6 py-6 sm:px-8">
      {favoriteError && <p role="alert" className="mb-5 rounded-lg border border-red-400/30 p-3 text-sm">{t("radio.favoriteError")}</p>}
      {tab === "directory" && directory.phase === "error" && <div role="alert" className="mb-5 rounded-lg border border-amber-400/30 p-4 text-sm"><p>{t("radio.directoryError")}</p><button type="button" onClick={() => session.retry()} className="mt-3 underline">{t("radio.retry")}</button></div>}
      {loading && <p role="status" className="mb-5 flex items-center gap-2 text-sm text-[color:var(--theme-fg-muted,#999)]"><LoaderCircle size={16} className="animate-spin" />{t("radio.loading")}</p>}
      {!loading && stations.length === 0 && (tab === "favorites" ? favoritesLoaded : directory.phase === "ready") && <p className="py-16 text-center text-sm text-[color:var(--theme-fg-muted,#999)]">{t(tab === "favorites" ? "radio.emptyFavorites" : "radio.empty")}</p>}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2 2xl:grid-cols-3">{stations.map(station => {
        const current = status.current_media?.kind === "radio" && status.current_media.station_id === station.id;
        const playing = current && status.state === "play";
        const diagnostic = current && !(available && playing
          && (status.playback_error?.code === "output_busy" || status.playback_error?.code === "output_unavailable"))
          ? status.playback_error : null;
        const failed = playState?.id === station.id && playState.failed || !!diagnostic;
        const errorKey = diagnostic && diagnostic.code !== "playback_error"
          ? diagnostic.code === "format_unsupported" ? "radio.directError" : `player.playbackError.${diagnostic.code}`
          : "radio.playError";
        const busy = playState?.id === station.id && playState.busy;
        return <article key={station.id} data-radio-station={station.id} aria-current={current ? "true" : undefined} className={`rounded-2xl border p-4 ${playing ? "border-[color:var(--theme-border-accent,#e5a00d)] bg-[var(--theme-surface-accent,#252010)]" : "border-[color:var(--theme-border-soft,#303030)] bg-[var(--theme-surface-panel,#191919)]"}`}>
          <div className="flex gap-4"><StationLogo station={station} /><div className="min-w-0 flex-1"><h2 className="truncate text-sm font-semibold" title={station.name}>{station.name}</h2><p className="mt-1 truncate text-xs text-[color:var(--theme-fg-muted,#999)]">{station.manual ? t("radio.manual") : countryName(station.country_code)}</p><p className="mt-1 truncate text-xs text-[color:var(--theme-fg-muted,#999)]" title={station.tags.join(" · ")}>{station.tags.slice(0, 3).join(" · ") || t("radio.noGenre")}</p></div>
            {station.manual && tab === "favorites" ? <div className="flex shrink-0 gap-1 self-start">
              <button type="button" disabled={favoriteBusy} onClick={() => openManual(station)} aria-label={t("radio.editStation", { name: station.name })} title={t("radio.edit")} className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#999)] hover:text-[color:var(--theme-fg-primary,#eee)]"><Pencil size={16} /></button>
              <button type="button" disabled={favoriteBusy} onClick={() => void save(() => radioService.favorite(station.id, false))} aria-label={t("radio.deleteStation", { name: station.name })} title={t("radio.delete")} className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#999)] hover:text-red-300"><Trash2 size={16} /></button>
            </div> : <button type="button" disabled={favoriteBusy} onClick={() => void save(() => radioService.favorite(station.id, !favoriteIds.has(station.id)))} aria-label={t(favoriteIds.has(station.id) ? "radio.unfavorite" : "radio.favorite", { name: station.name })} aria-pressed={favoriteIds.has(station.id)} className="self-start rounded-md p-1 text-[color:var(--theme-fg-accent,#e5a00d)]"><Heart size={17} fill={favoriteIds.has(station.id) ? "currentColor" : "none"} /></button>}
          </div><div className="mt-4 flex items-center justify-between gap-3"><span className="truncate text-[11px] text-[color:var(--theme-fg-muted,#999)]">{[station.codec, station.bitrate ? `${station.bitrate} kbps` : ""].filter(Boolean).join(" · ") || t("radio.live")}</span>
            <button type="button" disabled={!active || !available || !!playOwner.current} onClick={() => void (playing ? stop() : play(station))} aria-label={t(playing ? "radio.stopStation" : "radio.playStation", { name: station.name })} className="flex items-center gap-2 rounded-full bg-[var(--theme-control-accent,#E5A00D)] px-4 py-2 text-xs font-semibold text-black disabled:opacity-40">{busy ? <LoaderCircle size={14} className="animate-spin" /> : playing ? <Square size={13} fill="currentColor" /> : <Play size={14} fill="currentColor" />}{t(playing ? "radio.stop" : "radio.listen")}</button></div>
          {playing && <p role="status" className="mt-3 text-xs text-[color:var(--theme-fg-accent,#e5a00d)]">● {t("radio.live")}{status.title !== station.name ? ` · ${status.title}` : ""}</p>}
          {failed && <p role="alert" className="mt-3 text-xs text-red-300">{t(errorKey)}</p>}
        </article>;
      })}</div>
      {tab === "directory" && directory.phase === "ready" && (directory.hasMore || directory.query.offset > 0) && <div className="mt-6 flex items-center justify-center gap-5">{directory.query.offset > 0 && <button type="button" onClick={() => setQuery({ ...directory.query, offset: Math.max(0, directory.query.offset - RADIO_PAGE_SIZE) })} className="rounded-lg border border-[color:var(--theme-border-soft,#444)] px-6 py-2.5 text-sm">{t("radio.previous")}</button>}<span className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.page", { number: directory.query.offset / RADIO_PAGE_SIZE + 1 })}</span>{directory.hasMore && <button type="button" onClick={() => setQuery({ ...directory.query, offset: directory.query.offset + RADIO_PAGE_SIZE })} className="rounded-lg border border-[color:var(--theme-border-soft,#444)] px-6 py-2.5 text-sm">{t("radio.more")}</button>}</div>}
      {tab === "directory" && <p className="mt-8 text-center text-[11px] text-[color:var(--theme-fg-muted,#777)]">{t("radio.directoryCredit")} <button type="button" onClick={() => void openUrl("https://www.radio-browser.info/")} className="underline">Radio Browser</button></p>}
    </div>
    {manualOpen && <dialog ref={manualDialog} aria-labelledby="radio-manual-title" onCancel={event => { event.preventDefault(); if (!favoriteBusy) setManualOpen(false); }} onClick={event => { if (event.target === event.currentTarget && !favoriteBusy) setManualOpen(false); }} className="m-auto w-[calc(100%-40px)] max-w-lg rounded-2xl bg-transparent p-0 text-[color:var(--theme-fg-primary,#eee)] backdrop:bg-black/70">
      <form onSubmit={event => { event.preventDefault(); void save(() => manualId ? radioService.edit(manualId, manual.name, manual.url) : radioService.manual(manual.name, manual.url), true); }} onKeyDown={event => { event.stopPropagation(); if (event.key === "Escape" && !favoriteBusy) { event.preventDefault(); setManualOpen(false); } }} className="w-full max-w-lg rounded-2xl border border-[color:var(--theme-border-soft,#444)] bg-[var(--theme-surface-panel,#191919)] p-6 shadow-2xl">
        <div className="mb-5 flex justify-between gap-3"><h2 id="radio-manual-title" className="text-lg font-semibold">{t(manualId ? "radio.edit" : "radio.addManual")}</h2><button type="button" disabled={favoriteBusy} aria-label={t("radio.close")} onClick={() => setManualOpen(false)}><X size={20} /></button></div>
        <label className="block text-sm">{t("radio.name")}<input autoFocus required maxLength={200} value={manual.name} onChange={event => setManual({ ...manual, name: event.target.value })} className="mt-2 mb-4 block w-full rounded-lg border border-[color:var(--theme-border-soft,#444)] bg-transparent px-3 py-2.5" /></label>
        <label className="block text-sm">{t("radio.streamUrl")}<input type="url" required maxLength={4096} value={manual.url} onChange={event => setManual({ ...manual, url: event.target.value })} placeholder="https://" className="mt-2 block w-full rounded-lg border border-[color:var(--theme-border-soft,#444)] bg-transparent px-3 py-2.5" /></label><p className="mt-3 text-xs text-[color:var(--theme-fg-muted,#999)]">{t("radio.manualHint")}</p>
        {favoriteError && <p role="alert" className="mt-4 text-sm text-red-300">{t("radio.manualError")}</p>}<button type="submit" disabled={favoriteBusy} className="mt-6 rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-5 py-2.5 text-sm font-semibold text-black disabled:opacity-40">{t(manualId ? "radio.saveChanges" : "radio.save")}</button>
      </form>
    </dialog>}
  </section>;
}
