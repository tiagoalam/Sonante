import React, { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { audioService } from "../services/audio";
import { SourceAvailabilityReview } from "../utils/sourceAvailabilityReview";
import { VirtualList } from "./VirtualList";

export const LocalSourceAvailabilityPanel: React.FC<{
  context: object;
  blocked: boolean;
}> = ({ context, blocked }) => {
  const { t } = useTranslation();
  const review = useMemo(() => new SourceAvailabilityReview(audioService.getLocalSourceAvailability), []);
  const state = useSyncExternalStore(review.subscribe, review.getSnapshot);
  const currentContext = blocked ? null : context;
  const [rowsContainer, setRowsContainer] = useState<HTMLDivElement | null>(null);
  useEffect(() => {
    review.setContext(currentContext);
    return () => review.setContext(null);
  }, [review, currentContext]);
  // Hide old context rows synchronously, before effect cleanup can run.
  const observation = state.context === currentContext ? state.observation : null;
  const phase = state.context === currentContext ? state.phase : "idle";
  const rows = observation?.sources ?? [];
  const key = "localBrowser.sourceAvailability.";
  return <section aria-label={t(key + "title")} className="mb-6 rounded-lg border border-[color:var(--theme-border-soft,#333333)] p-3 text-xs">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <h3 className="font-semibold">{t(key + "title")}</h3>
      <button type="button" disabled={blocked || state.pending} onClick={() => {
        review.setContext(currentContext);
        void review.check();
      }} className="rounded border border-[color:var(--theme-border-soft,#333333)] px-3 py-1.5 disabled:opacity-50 focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">{t(key + "refresh")}</button>
    </div>
    <p className="mt-2 text-[color:var(--theme-fg-secondary,#BBBBBB)]">{t(key + "scope")}</p>
    <p role="status" aria-live="polite" className="mt-2">{t(key + (blocked ? "blocked" : state.pending ? "loading" : phase === "failed" ? "failed" : observation ? "observed" : "idle"))}</p>
    {observation?.observed_conflict && <p role="alert" className="mt-2">{t(key + "conflict")}</p>}
    {observation && rows.length === 0 && <p className="mt-2">{t(key + "noRows")}</p>}
    {rows.length > 0 && <div ref={setRowsContainer} className="mt-2 overflow-y-auto" style={{ height: Math.min(rows.length, 4) * 76 }}>
      <VirtualList items={rows} rowHeight={76} scrollContainer={rowsContainer}
        getItemKey={row => row.source_id} renderItem={row => <div className="h-full border-t border-[color:var(--theme-border-soft,#333333)] py-2">
          <p className="truncate font-semibold" title={row.label}>{row.label}</p>
          <p className="truncate" title={t(key + "status." + row.status)}>{t(key + "status." + row.status)}</p>
          {row.reason && <p className="truncate text-[color:var(--theme-fg-secondary,#BBBBBB)]" title={t(key + "reason." + row.reason)}>{t(key + "reason." + row.reason)}</p>}
        </div>} />
    </div>}
  </section>;
};
