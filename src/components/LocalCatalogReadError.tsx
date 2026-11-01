import { RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { canRetryLocalCatalog, localCatalogErrorKey, type LocalAlbumCatalogState } from "../utils/localAlbumCatalog";

export function LocalCatalogReadError({ catalog, updating, available, onRetry }: {
  catalog: LocalAlbumCatalogState;
  updating: boolean;
  available: boolean;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  if (!catalog.error) return null;
  const key = localCatalogErrorKey(catalog.error);
  return <div role="alert" className="flex flex-wrap items-center gap-3 border-b border-[color:var(--theme-border-soft,#222222)] px-8 py-3 text-xs text-[color:var(--theme-fg-error,var(--color-red-400))]">
    <p>{key ? t(key) : catalog.error}</p>
    {catalog.error !== "SOURCE_AVAILABILITY_UNSUPPORTED_FORMAT" && <button type="button"
      disabled={!canRetryLocalCatalog(catalog, updating, available)} onClick={onRetry}
      className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#444)] px-3 py-2 text-[color:var(--theme-fg-primary,#eee)] hover:bg-[var(--theme-surface-raised,#252525)] disabled:cursor-not-allowed disabled:opacity-40">
      <RefreshCw size={14} aria-hidden="true" />{t("localBrowser.retryCatalog")}
    </button>}
  </div>;
}
