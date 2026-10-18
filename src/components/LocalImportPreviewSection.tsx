import { Disc3, LoaderCircle } from "lucide-react";
import { useState } from "react";
import { VirtualAlbumGrid } from "./VirtualAlbumGrid";
import type { PreviewAlbum } from "../types/localImportPreview";
import { safeImportPreviewImage } from "../utils/localImportPreview";

function PreviewImage({ image }: { image: string }) {
  const [failed, setFailed] = useState(false);
  return failed ? null : (
    <img src={image} alt="" aria-hidden="true" draggable={false}
      className="absolute inset-0 w-full h-full object-cover" onError={() => setFailed(true)} />
  );
}

/** Presentation only; deliberately has no media services or action callbacks. */
export function LocalImportPreviewSection({ albums, scrollContainer, t }: {
  albums: readonly PreviewAlbum[];
  scrollContainer: HTMLElement | null;
  t: (key: string) => string;
}) {
  if (albums.length === 0) return null;
  return (
    <section aria-label={t("localBrowser.importPreviewTitle")} className="mt-8 first:mt-0">
      <h2 className="text-sm font-bold text-[color:var(--theme-fg-accent,#E5A00D)] mb-2">{t("localBrowser.importPreviewTitle")}</h2>
      <p className="text-xs text-[color:var(--theme-fg-muted,#888888)] mb-4">{t("localBrowser.importPreviewDescription")}</p>
      <VirtualAlbumGrid
        items={albums}
        getItemKey={album => album.provisional_group_key}
        scrollContainer={scrollContainer}
        renderItem={album => {
          const image = safeImportPreviewImage(album.image);
          return (
            <div className="min-w-0 flex flex-col">
              <div className="relative aspect-square w-full rounded-lg overflow-hidden mb-2.5 shadow-md bg-[var(--theme-surface-raised,#202020)] flex items-center justify-center">
                <Disc3 size={48} className="text-[color:var(--theme-fg-faint,#555555)]" aria-hidden="true" />
                {image && <PreviewImage key={image} image={image} />}
                <div data-theme-surface="contrast" className="absolute inset-0 bg-[var(--theme-surface-subtle,rgb(0_0_0_/_0.4))] flex flex-col items-center justify-center gap-2 text-[color:var(--theme-fg-accent,#E5A00D)]">
                  <LoaderCircle size={22} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
                  <span className="text-xs">{t("localBrowser.importPreviewIndexing")}</span>
                </div>
              </div>
              <p className="text-sm font-semibold text-[color:var(--theme-fg-secondary,#BBBBBB)] truncate">{album.album ?? t("localBrowser.importPreviewUnknownAlbum")}</p>
              <p className="text-xs text-[color:var(--theme-fg-muted,#888888)] truncate mt-0.5">{album.artist ?? t("localBrowser.importPreviewUnknownArtist")}</p>
            </div>
          );
        }}
      />
    </section>
  );
}
