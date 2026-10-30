import { useLayoutEffect, useState } from "react";
import type { PlaybackStatus } from "../types/audio";
import type { LocalAlbumCatalogState } from "./localAlbumCatalog";
import type { LocalAlbum } from "../types/local";
import { currentMediaKey, localAlbumForMedia } from "./playerNavigation";
import { localArtworkResolver, localAlbumThumbnails } from "./localArtworkSession";
import { localPlaybackThumbnail } from "./localPlaybackThumbnail";
import { audioService } from "../services/audio";
import { logError } from "./frontendDiagnostics";

function coverDirectory(status: PlaybackStatus): string | null {
  const media = status.current_media;
  if (media?.kind !== "local" || /[\0\r\n]/.test(media.uri) || media.uri.includes("://")) return null;
  const parts = media.uri.split("/");
  if (parts.length < 2 || parts.some(part => !part || part === "." || part === "..")) return null;
  return parts.slice(0, -1).join("/");
}

// Presentation only. Raw telemetry, logical queue and native queue cache stay untouched.
export function useLocalPlaybackArtwork(status: PlaybackStatus, catalog: LocalAlbumCatalogState,
  available: boolean, active: boolean, onlineEnabled: boolean, ensureCatalog: () => void): PlaybackStatus {
  const key = currentMediaKey(status.current_media);
  const directory = coverDirectory(status);
  const album = localAlbumForMedia(status.current_media, catalog.albums);
  const version = catalog.albums;
  // A cold catalog load is not an index update and cannot hold the player
  // artwork hostage. The existing source-scoped IPC still validates ownership,
  // current configuration and the target at every filesystem boundary.
  const blocked = !active || !available || status.is_updating || catalog.saveBlocked
    || catalog.mpdBusy || !!catalog.error || (catalog.loaded && catalog.loading);
  const [resolved, setResolved] = useState<{ key: string; version: readonly LocalAlbum[]; thumb: string } | null>(null);
  useLayoutEffect(() => {
    if (blocked || !directory || status.thumb || status.plex_image) return;
    let current = true;
    const publish = (thumb: string) => { if (current) setResolved({ key, version, thumb }); };
    const unsubscribe = album ? localArtworkResolver.subscribe(album.id, publish) : () => {};
    void (async () => {
      // Existing Local contract includes folder/disc and embedded artwork; no online search.
      let cover: string | null = null;
      if (album) {
        cover = await localArtworkResolver.resolve(album, false, () => !current, undefined, () => current);
      } else {
        // A virtual CUE directory retains folder-cover priority before its
        // embedded picture, just like the existing album/disc resolver.
        const paths = /\.cue$/i.test(directory)
          ? [directory.slice(0, directory.lastIndexOf("/")), directory] : [directory];
        for (const path of paths) {
          if (!current) return;
          cover = await audioService.getPlayerLocalCover(path);
          if (cover) break;
        }
      }
      if (!current) return;
      if (cover) publish(cover);
      else if (onlineEnabled && album) {
        const cached = await audioService.getCachedOnlineAlbumCover(album);
        if (cached) publish(cached);
      } else if (onlineEnabled && !album && !catalog.loaded) {
        // Only cache-only enrichment that needs an album identity may request
        // the saved MPD catalog after the focal local lookup found no picture.
        ensureCatalog();
      }
    })().catch(error => { if (current) logError("artwork.album-load", error); });
    return () => { current = false; unsubscribe(); };
  }, [key, directory, album, version, blocked, onlineEnabled, status.thumb, status.plex_image, catalog.loaded, ensureCatalog]);
  // Blocking suspends new reads, not an image already resolved for this media/catalog.
  if (status.thumb || status.plex_image) return status;
  // Existing saved browser rows may supply a preview on the first run after
  // upgrading too. They never become a confirmed album or authorize an IPC.
  const previewAlbum = album ?? localAlbumForMedia(status.current_media, catalog.presentationAlbums ?? []);
  const preview = localPlaybackThumbnail.get(status.current_media)
    ?? (previewAlbum ? localAlbumThumbnails.get(previewAlbum) : null);
  const thumb = resolved?.key === key && resolved.version === version ? resolved.thumb : preview;
  return thumb ? { ...status, thumb } : status;
}
