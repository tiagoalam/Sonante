import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { CircleAlert, X } from "lucide-react";
import { useIndexCoverage } from "../services/indexCoverage";
import { IndexCoverageDetails } from "./LocalIndexCoverage";

export function LocalWatchNotice({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t } = useTranslation();
  const [needsCheck, setNeedsCheck] = useState(false);
  const coverage = useIndexCoverage();
  const [dismissedCoverage, setDismissedCoverage] = useState<number | null>(null);
  const partial = coverage.report?.outcome === "partial" && dismissedCoverage !== coverage.revision;
  useEffect(() => {
    let disposed = false;
    let revision = 0;
    const cleanups: (() => void)[] = [];
    const observe = (value: boolean) => { revision++; if (!disposed) setNeedsCheck(value); };
    void (async () => {
      for (const [event, value] of [["local-library-watch-needs-check", true], ["local-library-watch-ready", false]] as const) {
        const cleanup = await listen(event, () => observe(value));
        if (disposed) { cleanup(); return; }
        cleanups.push(cleanup);
      }
      const sampled = revision;
      const current = await invoke<boolean>("get_local_watch_needs_check");
      if (!disposed && sampled === revision) setNeedsCheck(current);
    })().catch(() => { /* Optional notice must not interfere with playback. */ });
    return () => { disposed = true; for (const cleanup of cleanups) cleanup(); };
  }, []);
  if (!needsCheck && !partial) return null;
  return (
    <div role="alert" className="fixed bottom-28 left-6 z-40 flex max-w-sm items-start gap-2 rounded-lg border border-[color:var(--theme-border-accent,var(--color-amber-700))] bg-[var(--theme-surface-panel,#1D1D1D)] p-3 text-xs text-[color:var(--theme-fg-primary,#ffffff)] shadow-xl">
      <CircleAlert size={16} className="shrink-0 text-[color:var(--theme-fg-accent,var(--color-amber-300))]" />
      <div>
        {needsCheck && <p>{t("settings.localWatchNeedsCheck")}</p>}
        {partial && coverage.report && <><p>{t("indexCoverage.partial")}</p><IndexCoverageDetails report={coverage.report}/></>}
        <button type="button" onClick={onOpenSettings} className="mt-2 text-[color:var(--theme-fg-accent,#E5A00D)] underline">{t("settings.pages.local")}</button>
      </div>
      <button type="button" onClick={() => { setNeedsCheck(false); setDismissedCoverage(coverage.revision); }} aria-label={t("settings.dismissStatus")}><X size={14} /></button>
    </div>
  );
}
