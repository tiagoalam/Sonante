import { audioService } from "../services/audio";
import { artworkProgressService } from "../services/artworkProgress";
import { LocalArtworkResolver } from "./localArtwork";
import { LocalArtworkEnrichment } from "./localArtworkEnrichment";

export const localArtworkResolver = new LocalArtworkResolver(
  audioService.getLocalCover,
  audioService.getOnlineAlbumCover,
  150,
  audioService.getOnlineCoverCacheStatus,
);

export const localArtworkEnrichment = new LocalArtworkEnrichment(localArtworkResolver, undefined, undefined, artworkProgressService);
