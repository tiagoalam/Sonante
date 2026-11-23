import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { LYRICS_PREFERENCE_EVENT, readLyricsOnlinePreference, writeLyricsOnlinePreference } from "../utils/trackLyrics";

export function LyricsOnlinePreference({ disabled = false }: { disabled?: boolean }) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(readLyricsOnlinePreference);
  useEffect(() => {
    if (typeof window === "undefined" || !window.addEventListener) return;
    const sync = () => setEnabled(readLyricsOnlinePreference());
    window.addEventListener(LYRICS_PREFERENCE_EVENT, sync);
    window.addEventListener("storage", sync);
    return () => { window.removeEventListener(LYRICS_PREFERENCE_EVENT, sync); window.removeEventListener("storage", sync); };
  }, []);
  return <fieldset className="settings-section" data-settings-section="lyrics-online">
    <legend className="settings-section-title">{t("settings.lyricsOnline.title")}</legend>
    <label className="flex items-start gap-2 text-sm text-[color:var(--theme-fg-secondary,#bbb)]">
      <input type="checkbox" checked={enabled} disabled={disabled} onChange={event => {
        setEnabled(event.target.checked);
        writeLyricsOnlinePreference(event.target.checked);
      }} />
      <span>{t("settings.lyricsOnline.automatic")}</span>
    </label>
    <p className="settings-section-note text-[color:var(--theme-fg-muted,#888)]">{t("settings.lyricsOnline.privacy")}</p>
  </fieldset>;
}
