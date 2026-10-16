import { audioService } from "../services/audio";
import { LocalAlbumCatalog } from "./localAlbumCatalog";

// Shared by consumers within each webview; the detached window has its own session.
export const localAlbumCatalog = new LocalAlbumCatalog(audioService.getLocalAlbums);
