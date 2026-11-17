import React from "react";
import { Database, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SqliteIndexProgress } from "../services/sqliteLibrary";

type Props = {
  activity: SqliteIndexProgress | null;
  hidden?: boolean;
  onDismiss?: (operationId: string) => void;
};

export const LibraryActivityPanel: React.FC<Props> = ({ activity, hidden = false, onDismiss }) => {
  const { t } = useTranslation();
  if (!activity || hidden) return null;

  const complete = activity.phase === "completed";
  const failed = activity.phase === "failed";
  const hasKnownTotal = activity.phase === "metadata" && activity.metadataTotal > 0;
  const percent = hasKnownTotal
    ? Math.min(100, Math.floor(activity.metadataProcessed / activity.metadataTotal * 100))
    : complete ? 100 : null;
  const processedValue = activity.metadataTotal > 0
    ? `${Math.min(activity.metadataProcessed, activity.metadataTotal).toLocaleString()} / ${activity.metadataTotal.toLocaleString()}`
    : activity.phase === "scanning"
      ? activity.filesExamined.toLocaleString()
    : activity.phase === "grouping" || activity.phase === "persisting" || complete
      ? `${activity.audioFilesFound.toLocaleString()} / ${activity.audioFilesFound.toLocaleString()}`
      : "0";
  const filesLabel = t(activity.phase === "scanning"
    ? "sqliteLibrary.activity.filesExamined" : "sqliteLibrary.activity.filesProcessed");
  const phaseLabel = t(`sqliteLibrary.activity.phase.${activity.phase}`);
  const operationLabel = t(`sqliteLibrary.activity.operation.${activity.operation}`);
  const stateLabel = t(failed ? "sqliteLibrary.activity.state.failed" : complete
    ? "sqliteLibrary.activity.state.completed" : "sqliteLibrary.activity.state.running");

  return <section aria-labelledby="sqlite-library-activity-title" data-library-activity={activity.phase}
    className="flex h-full min-h-0 min-w-0 flex-col justify-center overflow-hidden rounded-lg border border-[color:var(--theme-border-soft,#303030)] bg-[var(--theme-surface-panel,#181818)] px-2 py-1 sm:px-2.5">
    <header className="flex min-h-0 min-w-0 items-center gap-1.5">
      <Database size={14} aria-hidden="true" className="shrink-0 text-[color:var(--theme-fg-muted,#aaaaaa)]" />
      <h3 id="sqlite-library-activity-title" className="sr-only sm:not-sr-only sm:min-w-0 sm:shrink-0 sm:truncate sm:text-[11px] sm:font-semibold sm:text-[color:var(--theme-fg-primary,#eeeeee)]">
        {t("sqliteLibrary.activity.title")}
      </h3>
      <span title={activity.sourceName} className="min-w-0 flex-1 truncate text-[10px] font-medium text-[color:var(--theme-fg-primary,#eeeeee)] sm:text-[11px]">
        {activity.sourceName}
      </span>
      <span role="status" aria-live="polite" className={`shrink-0 whitespace-nowrap rounded-full px-1.5 py-0.5 text-[9px] font-medium sm:px-2 sm:text-[10px] ${failed
        ? "bg-[color:var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-error,#f87171)]"
        : complete ? "bg-[color:var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-secondary,#cccccc)]"
          : "bg-[color:var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-accent,#E5A00D)]"}`}>
        {stateLabel}
      </span>
      {(complete || failed) && <button type="button" onClick={() => onDismiss?.(activity.operationId)} aria-label={t("sqliteLibrary.activity.dismiss")}
        className="shrink-0 rounded p-0.5 text-[color:var(--theme-fg-muted,#999999)] hover:bg-[var(--theme-surface-hover,#292929)]">
        <X size={13} />
      </button>}
    </header>

    <div className="mt-0.5 flex min-h-0 min-w-0 items-center gap-2 text-[9px] leading-3 text-[color:var(--theme-fg-secondary,#aaaaaa)] sm:text-[10px]">
      <p title={failed ? activity.errorMessage : `${operationLabel} · ${phaseLabel}`}
        className={`min-w-0 flex-1 truncate whitespace-nowrap ${failed ? "text-[color:var(--theme-fg-error,#f87171)]" : ""}`}>
        {failed ? activity.errorMessage : complete && activity.resultSummary
          ? t(`sqliteLibrary.activity.${activity.resultSummary}`, { tracks: activity.removedTrackCount ?? 0 }) : `${operationLabel} · ${phaseLabel}`}
      </p>
      <div className="flex max-w-[64%] shrink-0 items-center gap-1 whitespace-nowrap overflow-hidden text-ellipsis tabular-nums sm:gap-1.5">
        <span aria-label={`${filesLabel}: ${processedValue}`} title={`${filesLabel}: ${processedValue}`} className="truncate">{processedValue}</span>
        <span aria-hidden="true" className="shrink-0 text-[color:var(--theme-fg-muted,#666666)]">·</span>
        <span aria-label={`${t(activity.countsProvisional ? "sqliteLibrary.activity.albumsProvisional" : "sqliteLibrary.activity.albumsConfirmed")}: ${activity.identifiedAlbums}`}
          title={`${t(activity.countsProvisional ? "sqliteLibrary.activity.albumsProvisional" : "sqliteLibrary.activity.albumsConfirmed")}: ${activity.identifiedAlbums}`} className="shrink-0">
          {activity.countsProvisional ? "~" : ""}{activity.identifiedAlbums} {t("sqliteLibrary.activity.albumsShort")}
        </span>
        <span aria-hidden="true" className="shrink-0 text-[color:var(--theme-fg-muted,#666666)]">·</span>
        <span aria-label={`${t(activity.countsProvisional ? "sqliteLibrary.activity.artistsProvisional" : "sqliteLibrary.activity.artistsConfirmed")}: ${activity.identifiedArtists}`}
          title={`${t(activity.countsProvisional ? "sqliteLibrary.activity.artistsProvisional" : "sqliteLibrary.activity.artistsConfirmed")}: ${activity.identifiedArtists}`} className="shrink-0">
          {activity.countsProvisional ? "~" : ""}{activity.identifiedArtists} {t("sqliteLibrary.activity.artistsShort")}
        </span>
        {percent !== null && <span className="shrink-0 font-semibold">{percent}%</span>}
      </div>
    </div>
    <div className="mt-1 h-[3px] w-full overflow-hidden rounded-full bg-[var(--theme-surface-raised,#262626)]" aria-hidden={percent === null}>
      {percent !== null && <div role="progressbar" aria-label={t("sqliteLibrary.activity.progressLabel")}
        aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}
        className="h-full rounded-full bg-[color:var(--theme-fg-accent,#E5A00D)] transition-[width] duration-300 motion-reduce:transition-none"
        style={{ width: `${percent}%` }} />}
    </div>
  </section>;
};
