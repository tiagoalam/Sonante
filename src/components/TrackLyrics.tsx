import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CurrentMedia, PlaybackStatus } from "../types/audio";
import { audioService, type LocalLyricsLookup } from "../services/audio";
import { currentMediaKey } from "../utils/playerNavigation";
import { parseLrc, type LrcParseResult } from "../utils/lrcParser";
import { activeLrcEventIndex } from "../utils/lrcPlayback";
import { automaticLyricsMatches, hasSupportedLyricsIdentity, LYRICS_PREFERENCE_EVENT, lyricsFailureReason, lyricsKey, lyricsQuery, LyricsClient, readLyricsOnlinePreference, type LyricsRecord, type PersistedLyricsChoice, type LegacyLyricsCacheEntry } from "../utils/trackLyrics";

type LyricsViewState =
  | { key: string; kind: "loading" }
  | { key: string; kind: "found"; parsed: LrcParseResult }
  | { key: string; kind: "not_found" }
  | { key: string; kind: "unsupported" }
  | { key: string; kind: "error"; reason: "read" | "invalid" | "stale" };
type OnlineState = { key: string; searching: boolean; error: "offline" | "timeout" | "rate_limited" | "service" | "invalid" | null; saveFailed: boolean; records: LyricsRecord[]; selectedId: number | null; selection: "automatic" | "manual" | null; notFound: boolean; noEligible: boolean };
const trackLyrics = new LyricsClient((query, _signal, refresh) => audioService.searchOnlineLyrics({ ...query, cacheKey: lyricsKey(query) }, refresh), () => {
  try { return window.localStorage; } catch { return null; }
}, Date.now, (entries: LegacyLyricsCacheEntry[]) => audioService.migrateLegacyLyricsCache(entries));

function stableChoiceKey(media: CurrentMedia | null): string | null {
  if (!media || !hasSupportedLyricsIdentity(media)) return null;
  return media.kind === "local" ? JSON.stringify(["local", media.uri])
    : JSON.stringify(["plex", media.server_id, media.part_key]);
}

function decodeLookup(reply: LocalLyricsLookup, key: string): LyricsViewState {
  switch (reply.kind) {
    case "found": try { return { key, kind: "found", parsed: parseLrc(reply.content) }; } catch { return { key, kind: "error", reason: "invalid" }; }
    case "not_found": return { key, kind: "not_found" };
    case "not_eligible": return { key, kind: "unsupported" };
    case "invalid_content": return { key, kind: "error", reason: "invalid" };
    case "read_failed": return { key, kind: "error", reason: reply.reason === "stale_track" ? "stale" : "read" };
  }
}
function onlineContent(record: LyricsRecord): { kind: "instrumental" } | { kind: "found"; parsed: LrcParseResult } | { kind: "invalid" } {
  if (record.instrumental) return { kind: "instrumental" };
  if (record.syncedLyrics) {
    try { const parsed = parseLrc(record.syncedLyrics); if (parsed.kind === "synchronized") return { kind: "found", parsed }; } catch { /* Prefer a validated plain version below. */ }
  }
  if (record.plainLyrics) {
    try { return { kind: "found", parsed: parseLrc(record.plainLyrics) }; } catch { return { kind: "invalid" }; }
  }
  return { kind: "invalid" };
}
export const TrackLyrics: React.FC<{ track: PlaybackStatus | null }> = ({ track }) => {
  const { t } = useTranslation();
  const media: CurrentMedia | null = track?.current_media ?? null;
  const key = currentMediaKey(media);
  const choiceKey = stableChoiceKey(media);
  const snapshotReliable = track?.snapshot_consistent !== false && track?.playback_error?.code !== "queue_uncertain";
  const query = snapshotReliable ? lyricsQuery(track) : null;
  const localReadEligible = media?.kind === "local" && !media.uri.startsWith("sqlite-cue://");
  const onlineIdentityReady = snapshotReliable && hasSupportedLyricsIdentity(media);
  const onlineEligible = Boolean(onlineIdentityReady && query);
  const onlineSourceUnsupported = Boolean(media && media.kind !== "local" && media.kind !== "plex");
  const onlineKey = `${key}:${query ? lyricsKey(query) : ""}`;
  const [retry, setRetry] = useState(0);
  const [stored, setStored] = useState<LyricsViewState | null>(null);
  const [autoEnabled, setAutoEnabled] = useState(readLyricsOnlinePreference);
  const [online, setOnline] = useState<OnlineState | null>(null);
  const [choiceState, setChoiceState] = useState<{ key: string; choice: PersistedLyricsChoice | null; loaded: boolean; loadFailed: boolean; saveFailed: boolean } | null>(null);
  const [selectionOpen, setSelectionOpen] = useState(false);
  const [followPausedFor, setFollowPausedFor] = useState<string | null>(null);
  const lyricsContainerRef = useRef<HTMLDivElement | null>(null);
  const activeLineRef = useRef<HTMLParagraphElement | null>(null);
  const onlineSequence = useRef(0);
  const onlineAbort = useRef<AbortController | null>(null);
  const currentOnlineKey = useRef(onlineKey);
  const autoAttempted = useRef(new Set<string>());
  const autoInFlight = useRef(new Map<string, object>());
  const staleLocalContext = useRef<string | null>(null);
  const autoEnabledRef = useRef(autoEnabled);
  currentOnlineKey.current = onlineKey;
  autoEnabledRef.current = autoEnabled;

  useEffect(() => {
    let current = true;
    onlineAbort.current?.abort();
    onlineAbort.current = null;
    onlineSequence.current++;
    setOnline(null);
    setSelectionOpen(false);
    // Plex and CUE use metadata-only online lookup; neither is passed to the local sidecar reader.
    if (!snapshotReliable || !media || media.kind !== "local" || media.uri.startsWith("sqlite-cue://")) {
      if (!snapshotReliable && media) setStored({ key, kind: "loading" });
      return () => { current = false; };
    }
    setStored({ key, kind: "loading" });
    void audioService.getCurrentLocalLyrics(media).then(reply => {
      if (!current) return;
      const decoded = decodeLookup(reply, key);
      if (decoded.kind === "error" && decoded.reason === "stale") staleLocalContext.current = onlineKey;
      setStored(decoded);
    }).catch(() => { if (current) setStored({ key, kind: "error", reason: "read" }); });
    // Identity confidence is a meaningful read gate; elapsed is deliberately absent.
    return () => { current = false; onlineSequence.current++; onlineAbort.current?.abort(); onlineAbort.current = null; };
  }, [key, retry, snapshotReliable]);

  useEffect(() => {
    let current = true;
    if (!snapshotReliable || !media || !choiceKey) {
      setChoiceState(null);
      return () => { current = false; };
    }
    setChoiceState({ key: choiceKey, choice: null, loaded: false, loadFailed: false, saveFailed: false });
    const getter = audioService.getLyricsChoice;
    if (!getter) {
      setChoiceState({ key: choiceKey, choice: null, loaded: true, loadFailed: false, saveFailed: false });
      return () => { current = false; };
    }
    void getter(media).then(choice => {
      if (!current) return;
      const valid = choice && (choice.kind === "automatic" || choice.kind === "prefer_local"
        || choice.kind === "manual" && Boolean(onlineContent(choice.record) && choice.record.id > 0)) ? choice : null;
      setChoiceState({ key: choiceKey, choice: valid, loaded: true, loadFailed: false, saveFailed: false });
    }).catch(() => {
      if (current) setChoiceState({ key: choiceKey, choice: null, loaded: true, loadFailed: true, saveFailed: false });
    });
    return () => { current = false; };
  }, [choiceKey, snapshotReliable]);

  useEffect(() => {
    if (!snapshotReliable || !localReadEligible || stored?.key !== key || stored.kind !== "error" || stored.reason !== "stale"
      || staleLocalContext.current === onlineKey) return;
    // The same media identity can become usable after its metadata/query context advances.
    // Revalidate once for that context; elapsed-only snapshots do not change onlineKey.
    staleLocalContext.current = onlineKey;
    setRetry(value => value + 1);
  }, [key, onlineKey, snapshotReliable, localReadEligible, stored?.key, stored?.kind, stored?.kind === "error" ? stored.reason : null]);

  useEffect(() => {
    const requestKey = onlineKey;
    return () => {
      onlineSequence.current++;
      onlineAbort.current?.abort();
      onlineAbort.current = null;
      // A canceled request must not block the same identity if it becomes current again.
      autoInFlight.current.delete(requestKey);
      // Revisit must be able to restore the durable result from the backend cache.
      autoAttempted.current.delete(requestKey);
    };
  }, [onlineKey]);

  useEffect(() => {
    if (typeof window === "undefined" || !window.addEventListener) return;
    const syncPreference = () => setAutoEnabled(readLyricsOnlinePreference());
    window.addEventListener(LYRICS_PREFERENCE_EVENT, syncPreference);
    window.addEventListener("storage", syncPreference);
    return () => { window.removeEventListener(LYRICS_PREFERENCE_EVENT, syncPreference); window.removeEventListener("storage", syncPreference); };
  }, []);

  const localUnsupported = Boolean(media && !localReadEligible);
  const value = !media ? null : localUnsupported ? { key, kind: "unsupported" as const }
    : !snapshotReliable ? { key, kind: "loading" as const }
      : stored?.key === key ? stored : { key, kind: "loading" as const };
  const onlineValue = online?.key === onlineKey ? online : null;

  const runSearch = async (automatic: boolean, attemptToken?: object) => {
    if (!query || !onlineEligible) {
      if (attemptToken && autoInFlight.current.get(onlineKey) === attemptToken) autoInFlight.current.delete(onlineKey);
      return;
    }
    const requestedKey = onlineKey;
    onlineAbort.current?.abort();
    const controller = new AbortController();
    onlineAbort.current = controller;
    const sequence = ++onlineSequence.current;
    const previous = onlineValue;
    setOnline({ key: requestedKey, searching: true, error: null, saveFailed: previous?.saveFailed ?? false, records: previous?.records ?? [], selectedId: previous?.selectedId ?? null, selection: previous?.selection ?? null, notFound: false, noEligible: false });
    if (!automatic) setSelectionOpen(true);
    try {
      const result = await trackLyrics.lookup(query, controller.signal, !automatic && Boolean(previous?.records.length));
      if (currentOnlineKey.current !== requestedKey || onlineSequence.current !== sequence || automatic && !autoEnabledRef.current) return;
      if (automatic) autoAttempted.current.add(requestedKey);
      if (result.kind === "not_found") {
        setOnline(old => {
          const retain = !automatic && old?.key === requestedKey && old.records.length > 0;
          return { key: requestedKey, searching: false, error: null, saveFailed: old?.key === requestedKey ? old.saveFailed : false,
            records: retain ? old.records : [], selectedId: retain ? old.selectedId : null, selection: retain ? old.selection : null,
            notFound: !retain, noEligible: false };
        });
        return;
      }
      const ranked = automaticLyricsMatches(result.records, query);
      const best = automatic ? ranked[0] ?? null : null;
      const fetchedRecords = [...ranked, ...result.records.filter(record => !ranked.some(candidate => candidate.id === record.id))];
      const orderedRecords = !automatic && previous?.records.length
        ? [...fetchedRecords, ...previous.records.filter(record => !fetchedRecords.some(candidate => candidate.id === record.id))]
        : fetchedRecords;
      setOnline(old => ({ key: requestedKey, searching: false, error: null, records: orderedRecords,
        // Automatic adoption uses the first eligible ranked result; manual searches never replace the current lyric implicitly.
        selectedId: automatic ? best?.id ?? null : old?.key === requestedKey ? old.selectedId : null,
        selection: automatic ? best ? "automatic" : null : old?.key === requestedKey ? old.selection : null,
        saveFailed: result.persisted === false, notFound: false, noEligible: automatic && !best && result.records.length > 0 }));
      setSelectionOpen(automatic ? !best && result.records.length > 0 : result.records.length > 0);
    } catch (error) {
      const canceled = error instanceof Error && error.name === "AbortError";
      if (canceled) return;
      if (currentOnlineKey.current !== requestedKey || onlineSequence.current !== sequence || automatic && !autoEnabledRef.current) return;
      if (automatic) autoAttempted.current.add(requestedKey);
      const reason = lyricsFailureReason(error);
      setOnline(old => ({ key: requestedKey, searching: false, error: reason === "offline" ? "offline" : reason === "timeout" ? "timeout" : reason === "rate_limited" ? "rate_limited" : reason === "service" ? "service" : "invalid", saveFailed: old?.key === requestedKey ? old.saveFailed : false,
        records: old?.key === requestedKey ? old.records : [], selectedId: old?.key === requestedKey ? old.selectedId : null, selection: old?.key === requestedKey ? old.selection : null, notFound: false, noEligible: false }));
    } finally {
      if (attemptToken && autoInFlight.current.get(requestedKey) === attemptToken) autoInFlight.current.delete(requestedKey);
      if (onlineAbort.current === controller) onlineAbort.current = null;
    }
  };
  const runSearchRef = useRef(runSearch);
  runSearchRef.current = runSearch;

  useEffect(() => {
    const sourceHasNoLocalReader = localUnsupported;
    const savedChoice = choiceState?.key === choiceKey && choiceState.loaded ? choiceState.choice : null;
    if (!choiceState?.loaded || choiceState.loadFailed || choiceState.key !== choiceKey || savedChoice?.kind === "prefer_local" || savedChoice?.kind === "manual"
      || (!sourceHasNoLocalReader && value?.kind !== "not_found") || !autoEnabled || !onlineEligible || !query
      || autoAttempted.current.has(onlineKey) || autoInFlight.current.has(onlineKey)) return;
    const attemptToken = {};
    autoInFlight.current.set(onlineKey, attemptToken);
    void runSearchRef.current(true, attemptToken);
  }, [onlineKey, value?.kind, autoEnabled, onlineEligible, localUnsupported, choiceState?.key, choiceState?.loaded, choiceState?.choice?.kind, choiceKey]);

  const persistedChoice = choiceState?.key === choiceKey && choiceState.loaded ? choiceState.choice : null;
  const selectedRecord = persistedChoice?.kind === "manual" ? persistedChoice.record
    : onlineValue?.records.find(record => record.id === onlineValue.selectedId) ?? null;
  const chosen = selectedRecord && (persistedChoice?.kind === "manual" || onlineValue?.selection === "manual" || value?.kind !== "found") ? selectedRecord : null;
  const remote = chosen ? onlineContent(chosen) : null;
  const localVisible = value?.kind === "found" && !chosen && persistedChoice?.kind !== "manual";
  const displayed = chosen ? remote : value?.kind === "found" ? { kind: "found" as const, parsed: value.parsed }
    : null;
  // elapsed is the current queue item's clock (including a CUE segment); the range start is not added/subtracted here.
  const positionMs = track && Number.isFinite(track.elapsed) && track.elapsed >= 0 ? Math.floor(track.elapsed * 1_000) : 0;
  const activeIndex = displayed?.kind === "found" && displayed.parsed.kind === "synchronized" ? activeLrcEventIndex(displayed.parsed, positionMs) : null;
  const activeOrder = activeIndex !== null && displayed?.kind === "found" && displayed.parsed.kind === "synchronized" ? displayed.parsed.events[activeIndex].order : null;

  useEffect(() => {
    if (activeOrder === null || followPausedFor === key) return;
    const line = activeLineRef.current, container = lyricsContainerRef.current;
    if (!line || !container || !container.contains(line)) return;
    const reducedMotion = typeof window !== "undefined" && (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
    const containerRect = container.getBoundingClientRect();
    const lineRect = line.getBoundingClientRect();
    const top = container.scrollTop + lineRect.top - containerRect.top - container.clientHeight / 2 + lineRect.height / 2;
    container.scrollTo?.({ top, behavior: reducedMotion ? "auto" : "smooth" });
  }, [key, activeOrder, followPausedFor]);

  const markManualScroll = () => setFollowPausedFor(key);
  const retryRead = () => setRetry(value => value + 1);
  const persistChoice = async (choice: PersistedLyricsChoice) => {
    if (!media || !choiceKey) return;
    const setter = audioService.setLyricsChoice;
    setChoiceState({ key: choiceKey, choice, loaded: true, loadFailed: false, saveFailed: false });
    if (!setter) return;
    try {
      await setter(media, choice);
      if (stableChoiceKey(media) === choiceKey) setChoiceState(current => current?.key === choiceKey ? { ...current, saveFailed: false } : current);
    } catch {
      if (stableChoiceKey(media) === choiceKey) setChoiceState(current => current?.key === choiceKey ? { ...current, saveFailed: true } : current);
    }
  };
  const returnToLocal = () => {
    setOnline(old => old?.key === onlineKey ? { ...old, selectedId: null, selection: null } : old);
    void persistChoice({ kind: "prefer_local" });
  };
  const returnToAutomatic = () => {
    setOnline(old => old?.key === onlineKey ? { ...old, selectedId: null, selection: null, searching: false } : old);
    autoAttempted.current.delete(onlineKey);
    void persistChoice({ kind: "automatic" });
  };
  const chooseRecord = (record: LyricsRecord) => {
    setOnline(old => old?.key === onlineKey ? { ...old, selectedId: record.id, selection: "manual", notFound: false, noEligible: false } : old);
    setSelectionOpen(false);
    void persistChoice({ kind: "manual", record });
  };
  const localMessage = !media ? "noTrack" : !snapshotReliable ? "identityPending" : chosen ? null
      : onlineSourceUnsupported ? "unsupported" : choiceState?.key !== choiceKey || !choiceState.loaded ? "loading" : value?.kind === "loading" ? "loading"
      : value?.kind === "not_found" && !chosen && !(autoEnabled && onlineEligible) ? "notFound"
        : value?.kind === "error" ? value.reason === "invalid" ? "invalid" : value.reason === "stale" ? "stale" : "readError" : null;
  const onlineMessage = onlineValue?.searching ? "searching"
    : onlineValue?.error ? `online${onlineValue.error === "offline" ? "Offline" : onlineValue.error === "timeout" ? "Timeout" : onlineValue.error === "rate_limited" ? "RateLimited" : onlineValue.error === "service" ? "Service" : "Invalid"}`
      : onlineValue?.noEligible ? "onlineNoEligible" : onlineValue?.notFound ? "onlineNotFound"
        : onlineIdentityReady && !query && (localUnsupported || value?.kind === "not_found" || value?.kind === "error") ? "metadataNeeded" : null;

  return <section className="np-lyrics" aria-label={t("nowPlaying.lyrics.title")} data-state={value?.kind ?? "idle"}>
    <div className="np-lyrics-heading"><span className="np-notes-label">{t("nowPlaying.lyrics.title")}</span>
      <span className="np-lyrics-actions">
        {value?.kind === "error" && <button type="button" onClick={retryRead}>{t("nowPlaying.lyrics.retry")}</button>}
        {onlineEligible && <button type="button" onClick={() => void runSearch(false)} disabled={Boolean(onlineValue?.searching)}>{t(chosen || localVisible ? "nowPlaying.lyrics.searchOther" : "nowPlaying.lyrics.searchOnline")}</button>}
        {chosen && localVisible === false && value?.kind === "found" && <button type="button" onClick={returnToLocal}>{t("nowPlaying.lyrics.returnLocal")}</button>}
        {persistedChoice && persistedChoice.kind !== "automatic" && <button type="button" onClick={returnToAutomatic}>{t("nowPlaying.lyrics.useAutomatic")}</button>}
        {displayed?.kind === "found" && displayed.parsed.kind === "synchronized" && followPausedFor === key && <button type="button" onClick={() => setFollowPausedFor(null)}>{t("nowPlaying.lyrics.resumeFollow")}</button>}
      </span>
    </div>
    {localMessage && <p role="status" className="np-lyrics-hint">{t(`nowPlaying.lyrics.${localMessage}`)}</p>}
    {onlineMessage && <p role="status" className="np-lyrics-hint">{t(`nowPlaying.lyrics.${onlineMessage}`)}</p>}
    {choiceState?.key === choiceKey && choiceState.loadFailed && <p role="status" className="np-lyrics-hint">{t("nowPlaying.lyrics.restoreFailed")}</p>}
    {(onlineValue?.saveFailed || choiceState?.key === choiceKey && choiceState.saveFailed) && <p role="status" className="np-lyrics-hint">{t("nowPlaying.lyrics.saveFailed")}</p>}
    {selectionOpen && onlineValue?.records.length ? <div className="np-lyrics-candidates" aria-label={t("nowPlaying.lyrics.candidates")}>
      {onlineValue.records.map(record => <button type="button" key={record.id} aria-pressed={record.id === onlineValue.selectedId}
        onClick={() => chooseRecord(record)}>
        <strong>{record.trackName}</strong><span>{record.artistName} · {record.albumName || t("nowPlaying.lyrics.unknownAlbum")} · {t("nowPlaying.lyrics.durationSeconds", { seconds: Math.round(record.duration) })}</span>
        <small>{record.instrumental ? t("nowPlaying.lyrics.instrumental") : record.syncedLyrics ? t("nowPlaying.lyrics.syncedAvailable") : record.plainLyrics ? t("nowPlaying.lyrics.plainAvailable") : t("nowPlaying.lyrics.noLyrics")}</small>
      </button>)}
    </div> : null}
    {chosen && remote?.kind === "instrumental" && <p role="status" className="np-lyrics-hint">{t("nowPlaying.lyrics.instrumental")}</p>}
    {chosen && remote?.kind === "invalid" && <p role="status" className="np-lyrics-hint">{t("nowPlaying.lyrics.onlineInvalid")}</p>}
    {displayed?.kind === "found" && <small className="np-lyrics-credit">{t(chosen ? "nowPlaying.lyrics.sourceOnline" : "nowPlaying.lyrics.sourceLocal")} · {t(displayed.parsed.kind === "synchronized" ? "nowPlaying.lyrics.syncedType" : "nowPlaying.lyrics.noSync")}</small>}
    {chosen && remote?.kind === "instrumental" && <small className="np-lyrics-credit">{t("nowPlaying.lyrics.sourceOnline")} · {t("nowPlaying.lyrics.instrumental")}</small>}
    {displayed?.kind === "found" && displayed.parsed.kind === "plain" && <div className="np-lyrics-text" tabIndex={0}>{displayed.parsed.text}</div>}
    {displayed?.kind === "found" && displayed.parsed.kind === "synchronized" && <div ref={lyricsContainerRef} className="np-lyrics-text np-lyrics-synchronized"
      tabIndex={0} aria-label={t("nowPlaying.lyrics.title")} onWheel={markManualScroll} onTouchStart={markManualScroll} onPointerDown={markManualScroll} onKeyDown={markManualScroll}>
      {displayed.parsed.events.map((event, index) => <p key={`${chosen?.id ?? "local"}-${event.order}`} ref={index === activeIndex ? activeLineRef : null}
        className={index === activeIndex ? "np-lyrics-line is-active" : "np-lyrics-line"} aria-current={index === activeIndex ? "true" : undefined}>{event.text || "\u00a0"}</p>)}
    </div>}
  </section>;
};
