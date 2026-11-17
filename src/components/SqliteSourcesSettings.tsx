import React, { useState, useSyncExternalStore } from "react";
import { HardDrive, Search, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { Source, SqliteIndexProgress } from "../services/sqliteLibrary";
import { localArtworkEnrichment } from "../utils/localArtworkSession";

interface Props {
  sources: Source[];
  progress: SqliteIndexProgress | null;
  busy: boolean;
  onAddFolder: () => void;
  onVerify: (sourceId: string) => Promise<boolean>;
  onReindex: (sourceId: string) => Promise<void>;
  onRemove: (sourceId: string) => Promise<void>;
  onlineArtworkEnabled: boolean;
  onOnlineArtworkEnabledChange: (enabled: boolean) => void;
  onSearchMissingArtwork: () => void;
  artworkCatalogReady: boolean;
}

/** Settings entry point for the same SQLite source operations used by Local. */
export const SqliteSourcesSettings: React.FC<Props> = ({
  sources, progress, busy, onAddFolder, onVerify, onReindex, onRemove,
  onlineArtworkEnabled = false, onOnlineArtworkEnabledChange = () => {},
  onSearchMissingArtwork = () => {}, artworkCatalogReady = false,
}) => {
  const { t } = useTranslation();
  const artworkSearch = useSyncExternalStore(localArtworkEnrichment.subscribe, localArtworkEnrichment.getSnapshot, localArtworkEnrichment.getSnapshot);
  const [confirmRemoval, setConfirmRemoval] = useState<number | null>(null);
  const operationError = progress?.phase === "failed" ? progress.errorMessage : null;

  return <section data-settings-section="sqlite-sources" className="settings-section" aria-label={t("sqliteLibrary.sources")}>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <h3 className="settings-section-title">
        <HardDrive size={14} />{t("sqliteLibrary.sources")}
      </h3>
      <button type="button" onClick={onAddFolder} disabled={busy}
        className="rounded bg-[#E5A00D] px-3 py-1.5 text-xs font-bold text-black disabled:cursor-not-allowed disabled:opacity-50">
        {busy ? t("sqliteLibrary.indexing") : t("sqliteLibrary.addFolder")}
      </button>
    </div>

    {operationError && <p role="alert" className="text-[11px] text-[color:var(--theme-fg-error,var(--color-red-400))]">{operationError}</p>}
    {progress && progress.phase !== "failed" && progress.phase !== "completed" &&
      <p role="status" className="text-[11px] text-[color:var(--theme-fg-secondary,#999)]">{progress.sourceName} · {t(`sqliteLibrary.activity.phase.${progress.phase}`)}</p>}

    <div className="max-h-56 space-y-2 overflow-y-auto rounded-lg border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-base,#121212)] p-2.5">
      {sources.length === 0 ? <p className="py-3 text-center text-[11px] text-[color:var(--theme-fg-muted,#777)]">{t("sqliteLibrary.openHint")}</p> : sources.map(source =>
        <article key={source.id} className="rounded-lg border border-[color:var(--theme-border-soft,#242424)] bg-[var(--theme-surface-panel,#181818)] p-3">
          <div className="flex min-w-0 items-start justify-between gap-3">
            <div className="min-w-0">
              <h4 className="truncate text-xs font-semibold text-[color:var(--theme-fg-primary,#fff)]" title={source.displayName}>{source.displayName}</h4>
              <p className="mt-1 text-[11px] text-[color:var(--theme-fg-muted,#888)]">
                {source.trackCount} {t("sqliteLibrary.sourceTrackCount")} · {t("sqliteLibrary.generation", { generation: source.generation })}
              </p>
            </div>
            <div className="flex shrink-0 flex-wrap justify-end gap-1">
              <button type="button" disabled={busy} onClick={() => void onVerify(`sqlite-${source.id}`).catch(() => {})}
                className="rounded px-2 py-1 text-[11px] text-[color:var(--theme-fg-secondary,#bbb)] hover:bg-[var(--theme-surface-raised,#242424)] disabled:opacity-50">
                <RefreshCw size={12} className="mr-1 inline" />{t("sqliteLibrary.verifyChanges")}
              </button>
              <button type="button" disabled={busy} onClick={() => void onReindex(`sqlite-${source.id}`).catch(() => {})}
                title={t("sqliteLibrary.reindexSource")} className="rounded px-2 py-1 text-[11px] text-[color:var(--theme-fg-secondary,#bbb)] hover:bg-[var(--theme-surface-raised,#242424)] disabled:opacity-50">
                {t("sqliteLibrary.reindexShort")}
              </button>
              <button type="button" disabled={busy} onClick={() => setConfirmRemoval(source.id)}
                title={t("sqliteLibrary.removeSource")} className="rounded px-2 py-1 text-[11px] text-[color:var(--theme-fg-error,#f87171)] hover:bg-[var(--theme-surface-raised,#242424)] disabled:opacity-50">
                <Trash2 size={12} className="mr-1 inline" />{t("sqliteLibrary.removeShort")}
              </button>
            </div>
          </div>
          {confirmRemoval === source.id && <div role="alertdialog" aria-label={t("sqliteLibrary.removeSource")} className="mt-3 rounded border border-[color:var(--theme-border-soft,#333)] p-3 text-[11px]">
            <p>{t("sqliteLibrary.removeConfirm", { source: source.displayName })}</p>
            <div className="mt-2 flex justify-end gap-2">
              <button type="button" disabled={busy} onClick={() => setConfirmRemoval(null)} className="rounded px-2 py-1 hover:bg-[var(--theme-surface-raised,#242424)]">{t("settings.cancel")}</button>
              <button type="button" disabled={busy} onClick={() => {
                void onRemove(`sqlite-${source.id}`).then(() => setConfirmRemoval(null)).catch(() => {});
              }} className="rounded bg-[var(--theme-surface-raised,#242424)] px-2 py-1 text-[color:var(--theme-fg-error,#f87171)] disabled:opacity-50">{t("sqliteLibrary.removeShort")}</button>
            </div>
          </div>}
        </article>)
      }
    </div>
    <p className="text-[10px] leading-relaxed text-[color:var(--theme-fg-muted,#777)]">{t("sqliteLibrary.sourceRemovalFilesPreserved")}</p>

    <section className="settings-section" data-settings-section="sqlite-online-artwork" aria-label={t("settings.onlineArtwork")}>
      <h3 className="settings-section-title"><Search size={14} />{t("settings.onlineArtwork")}</h3>
      <label className="flex items-start gap-2 cursor-pointer">
        <input type="checkbox" checked={onlineArtworkEnabled} onChange={event => onOnlineArtworkEnabledChange(event.target.checked)}
          className="mt-0.5 accent-[var(--theme-control-accent,#E5A00D)]" />
        <span className="text-[11px] leading-relaxed text-[color:var(--theme-fg-secondary,#bbb)]">{t("settings.onlineArtworkDescription")}</span>
      </label>
      <div className="flex flex-wrap items-center gap-3">
        <button type="button" onClick={onSearchMissingArtwork}
          disabled={!onlineArtworkEnabled || !artworkCatalogReady || sources.length === 0 || busy || artworkSearch.active}
          className="flex items-center gap-1.5 rounded bg-[var(--theme-surface-raised,#242424)] px-2.5 py-1.5 text-[11px] text-[color:var(--theme-fg-secondary,#ccc)] hover:text-[color:var(--theme-fg-primary,#fff)] disabled:cursor-not-allowed disabled:opacity-50">
          <Search size={13} />{t("settings.searchMissingArtwork")}
        </button>
        {artworkSearch.active && <span role="status" className="text-[11px] text-[color:var(--theme-fg-secondary,#bbb)]">
          {t("settings.onlineArtworkProgress", { processed: Math.min(artworkSearch.processed, Math.max(0, artworkSearch.total - 1)), total: artworkSearch.total })}
          {artworkSearch.onlineFound > 0 ? ` · ${t("settings.onlineArtworkFound", { count: artworkSearch.onlineFound })}` : ""}
          {artworkSearch.failed > 0 ? ` · ${t("settings.onlineArtworkFailed", { count: artworkSearch.failed })}` : ""}
        </span>}
        {!artworkSearch.active && artworkSearch.outcome !== "idle" && artworkSearch.outcome !== "running" &&
          <span role="status" className="text-[11px] text-[color:var(--theme-fg-muted,#888)]">
            {t(artworkSearch.outcome === "completed" ? "settings.onlineArtworkCompleted"
              : artworkSearch.outcome === "partial" ? "settings.onlineArtworkPartial" : "settings.onlineArtworkCancelled", {
              processed: Math.min(artworkSearch.processed, artworkSearch.total), total: artworkSearch.total,
              found: artworkSearch.onlineFound, failed: artworkSearch.failed,
            })}
          </span>}
      </div>
    </section>
  </section>;
};
