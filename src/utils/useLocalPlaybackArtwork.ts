import { useEffect, useState } from "react";
import type { PlaybackStatus } from "../types/audio";
import type { LocalAlbumCatalogState } from "./localAlbumCatalog";
import type { LocalAlbum } from "../types/local";
import { currentMediaKey, localAlbumForMedia } from "./playerNavigation";
import { artworkReadBlocked } from "./localArtwork";
import { localArtworkResolver } from "./localArtworkSession";
import { audioService } from "../services/audio";
import { logError } from "./frontendDiagnostics";

// Presentation only. Raw telemetry, logical queue and persisted cache stay untouched.
export function useLocalPlaybackArtwork(status: PlaybackStatus, catalog: LocalAlbumCatalogState,
  available: boolean, active: boolean, onlineEnabled: boolean, ensureCatalog: () => void): PlaybackStatus {
  const key = currentMediaKey(status.current_media);
  const local = status.current_media?.kind === "local";
  const album = localAlbumForMedia(status.current_media, catalog.albums);
  const version = catalog.albums;
  const blocked = !active || !available || artworkReadBlocked(status.is_updating, catalog);
  const [resolved, setResolved] = useState<{ key: string; album: LocalAlbum; version: readonly LocalAlbum[]; thumb: string } | null>(null);
  useEffect(() => {
    if (active && available && local && !status.thumb && !status.plex_image && !catalog.loaded
      && !status.is_updating && !catalog.loading && !catalog.saveBlocked && !catalog.mpdBusy && !catalog.error) ensureCatalog();
  }, [active, available, local, status.thumb, status.plex_image, status.is_updating,
    catalog.loaded, catalog.loading, catalog.saveBlocked, catalog.mpdBusy, catalog.error, ensureCatalog]);
  useEffect(() => {
    if (blocked || !album || status.thumb || status.plex_image) return;
    let current = true;
    const publish = (thumb: string) => { if (current) setResolved({ key, album, version, thumb }); };
    const unsubscribe = localArtworkResolver.subscribe(album.id, publish);
    void (async () => {
      // Existing Local contract includes folder/disc and embedded artwork; no online search.
      const cover = await localArtworkResolver.resolve(album, false, () => !current, undefined, () => current);
      if (!current) return;
      if (cover) publish(cover);
      else if (onlineEnabled) {
        const cached = await audioService.getCachedOnlineAlbumCover(album);
        if (cached) publish(cached);
      }
    })().catch(error => { if (current) logError("artwork.album-load", error); });
    return () => { current = false; unsubscribe(); };
  }, [key, album, version, blocked, onlineEnabled, status.thumb, status.plex_image]);
  // Blocking suspends new reads, not an image already resolved for this media/catalog.
  return !status.thumb && !status.plex_image && resolved?.key === key
    && resolved.album === album && resolved.version === version ? { ...status, thumb: resolved.thumb } : status;
}
