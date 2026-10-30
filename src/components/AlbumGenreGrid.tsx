import { useTranslation } from "react-i18next";
import type { deriveAlbumGenres } from "../utils/albumGenres";

export function AlbumGenreGrid({ genres, onSelect, albumCountLabel }: {
  genres: ReturnType<typeof deriveAlbumGenres>;
  onSelect: (key: string) => void;
  albumCountLabel: (count: number) => string;
}) {
  const { t } = useTranslation();
  if (genres.length === 0) return <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">{t("common.emptyGenres")}</div>;
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-4">
      {genres.map(genre => (
        <button
          key={genre.value}
          type="button"
          onClick={() => onSelect(genre.value)}
          className="rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#181818)] p-5 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:bg-[var(--theme-surface-raised,#1E1E1E)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
        >
          <span className="block text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] break-words">{genre.label}</span>
          <span className="mt-1 block text-xs text-[color:var(--theme-fg-muted,#777777)]">{albumCountLabel(genre.albumCount)}</span>
        </button>
      ))}
    </div>
  );
}
