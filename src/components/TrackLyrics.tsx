import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PlaybackStatus } from "../types/audio";
import { LYRICS_ENABLED_KEY, lyricsKey, lyricsQuery, trackLyrics, lyricsFailureReason, type LyricsFailureReason, type LyricsResult } from "../utils/trackLyrics";
const readEnabled = () => { try { return window.localStorage.getItem(LYRICS_ENABLED_KEY) === "true"; } catch { return false; } };
export const TrackLyrics: React.FC<{ track: PlaybackStatus }> = ({ track }) => {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(readEnabled);
  const [result, setResult] = useState<{ key: string; value: LyricsResult | { kind: "error"; reason: LyricsFailureReason } } | null>(null);
  const [retry, setRetry] = useState(0);
  const query = lyricsQuery(track), key = query ? lyricsKey(query) : "";
  useEffect(() => {
    const sync = (event: StorageEvent) => { if (event.key === LYRICS_ENABLED_KEY || event.key === null) setEnabled(event.newValue === "true"); };
    window.addEventListener("storage", sync); return () => window.removeEventListener("storage", sync);
  }, []);
  useEffect(() => {
    setResult(null);
    if (!enabled || !key || !query) return;
    const controller = new AbortController();
    // Rapid skips do not send a request for every transient title.
    const timer = window.setTimeout(() => {
      void trackLyrics.lookup(query, controller.signal).then(value => {
        if (!controller.signal.aborted) setResult({ key, value });
      }).catch(error => {
        if (controller.signal.aborted) return;
        const reason = lyricsFailureReason(error);
        setResult({ key, value: { kind: "error", reason } });
      });
    }, 350);
    return () => { window.clearTimeout(timer); controller.abort(); };
    // Identity contains only the stable fields used in this request, never elapsed/audio telemetry.
  }, [enabled, key, retry]);
  const value = result?.key === key ? result.value : null;
  return <section data-enabled={enabled} className="np-lyrics" aria-label={t("nowPlaying.lyrics.title")}>
    <div className="np-lyrics-heading"><span className="np-notes-label">{t("nowPlaying.lyrics.title")}</span>
      <label><input type="checkbox" checked={enabled} onChange={event => {
        const next = event.target.checked; setEnabled(next);
        try { window.localStorage.setItem(LYRICS_ENABLED_KEY, String(next)); } catch { /* Session choice remains available. */ }
      }} />{t("nowPlaying.lyrics.enable")}</label></div>
    {!enabled ? <p className="np-lyrics-hint">{t("nowPlaying.lyrics.privacy")}</p>
      : !query ? <p className="np-lyrics-hint">{t("nowPlaying.lyrics.noMetadata")}</p>
      : value?.kind === "error" ? <div><p role="status" className="np-lyrics-hint">{t(`nowPlaying.lyrics.failure.${value.reason}`)}</p><button type="button" onClick={() => setRetry(value => value + 1)}>{t("nowPlaying.lyrics.retry")}</button></div>
      : !value ? <p role="status" className="np-lyrics-hint">{t("nowPlaying.lyrics.loading")}</p>
      : value.kind === "found" ? <><div className="np-lyrics-text" tabIndex={0} aria-label={t("nowPlaying.lyrics.title")}>{value.text}</div><small>{t("nowPlaying.lyrics.credit")}</small></>
      : <p role="status" className="np-lyrics-hint">{t(value.kind === "instrumental" ? "nowPlaying.lyrics.instrumental" : "nowPlaying.lyrics.notFound")}</p>}
  </section>;
};
