import { audioService } from "../services/audio";
import { artworkProgressService } from "../services/artworkProgress";
import { LocalArtworkResolver } from "./localArtwork";
import { LocalArtworkEnrichment } from "./localArtworkEnrichment";
import { LocalAlbumThumbnails } from "./localAlbumThumbnails";

export const localArtworkResolver = new LocalArtworkResolver(
  audioService.getLocalCover,
  audioService.getOnlineAlbumCover,
  150,
  audioService.getOnlineCoverCacheStatus,
);

export const localArtworkEnrichment = new LocalArtworkEnrichment(localArtworkResolver, undefined, undefined, artworkProgressService);

// Separate from the resolver: a saved thumbnail is never a resolved artwork hit.
export const localAlbumThumbnails = new LocalAlbumThumbnails(() => window.localStorage, undefined, undefined,
  import.meta.env?.DEV ? value => console.info("[Sonante] local.thumbnails", JSON.stringify(value)) : undefined);
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => localAlbumThumbnails.finishDiagnostics(), { once: true });
}
