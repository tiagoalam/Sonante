import { audioService } from "../services/audio";
import { LocalAlbumCatalog } from "./localAlbumCatalog";
import { createLocalAlbumPresentationCache } from "./localAlbumPresentationCache";

// Shared by consumers within each webview; the detached window has its own session.
export const localAlbumCatalog = new LocalAlbumCatalog(audioService.getLocalAlbums,
  createLocalAlbumPresentationCache(() => window.localStorage));
