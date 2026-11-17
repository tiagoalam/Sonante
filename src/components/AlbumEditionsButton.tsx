import { useExperimentalFeatures, BetaBadge } from "./ExperimentalFeatures";
import { Layers3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { openAlbumEditions, validEditionAnchor, type EditionAnchor, type EditionSource } from "../utils/albumEditions";
import { hasVerifiedEditionAlternative, shouldShowAlbumEditionsAction } from "../utils/albumEditionAvailability";
export function AlbumEditionsButton({ album, source, albumId, disabled = false }: { album: EditionAnchor; source?: EditionSource; albumId?: string; disabled?: boolean }) {
  const { t } = useTranslation();
  const enabled = useExperimentalFeatures();
  const hasAlternative = !!source && !!albumId && hasVerifiedEditionAlternative(source, albumId);
  if (!shouldShowAlbumEditionsAction(enabled, validEditionAnchor(album), hasAlternative)) return null;
  return <button type="button" disabled={disabled} onClick={() => openAlbumEditions({ ...album, source, album_id: albumId })}
    className="inline-flex items-center gap-2 rounded-xl border border-[color:var(--theme-border-soft,#333)] px-4 py-2.5 text-xs font-semibold hover:bg-[var(--theme-surface-hover,#292929)] disabled:opacity-40 focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"><Layers3 size={16} />{t('editions.albumAction')}<BetaBadge /></button>;
}
