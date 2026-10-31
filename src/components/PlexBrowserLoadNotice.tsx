import { RefreshCw, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PlexBrowserLoadState } from "../utils/plexBrowserLoadState";

export function PlexBrowserLoadNotice({ state, compact, onRetry }: {
  state: PlexBrowserLoadState | null;
  compact: boolean;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  if (!state) return null;
  const loading = state === "libraries_loading" || state === "catalog_loading";
  const failed = state === "libraries_error" || state === "catalog_error";
  return <div data-plex-load-state={state} role={failed ? "alert" : "status"} aria-busy={loading}
    className={`flex flex-col items-center justify-center gap-3 text-center text-sm text-[color:var(--theme-fg-secondary,#aaa)] ${compact ? "mb-6 rounded-xl border border-[color:var(--theme-border-soft,#333)] p-5" : "min-h-48 p-6"}`}>
    {loading ? <RefreshCw size={23} aria-hidden="true" className="animate-spin text-[color:var(--theme-fg-accent,#e5a00d)]" /> : <Server size={25} aria-hidden="true" className="text-[color:var(--theme-fg-accent,#e5a00d)]" />}
    <p>{t(state === "libraries_loading" ? "plex.loadingLibraries" : state === "libraries_error" ? "plex.librariesUnavailable"
      : state === "catalog_loading" ? "plex.loadingCatalog" : state === "catalog_error" ? "plex.catalogUnavailable" : "plex.noLibraries")}</p>
    {!loading && <button type="button" onClick={onRetry} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#444)] px-4 py-2 text-xs text-[color:var(--theme-fg-primary,#eee)] hover:bg-[var(--theme-surface-raised,#252525)] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#e5a00d)]"><RefreshCw size={14} aria-hidden="true" />{t("plex.retryLoad")}</button>}
  </div>;
}
