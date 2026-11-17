import { useLayoutEffect, useState } from "react";
import type { PlaybackStatus } from "../types/audio";
import type { LocalAlbumCatalogState } from "./localAlbumCatalog";
import type { LocalAlbum } from "../types/local";
import { currentMediaKey, localAlbumForMedia } from "./playerNavigation";
import { localArtworkResolver, localAlbumThumbnails } from "./localArtworkSession";
import { localPlaybackThumbnail } from "./localPlaybackThumbnail";
import { audioService } from "../services/audio";
import { sqliteLibrary } from "../services/sqliteLibrary";
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
  const sqliteTrackLocator = status.current_media?.kind === "local"
    && (status.current_media.uri.startsWith("sqlite-track://") || status.current_media.uri.startsWith("sqlite-cue://")) ? status.current_media.uri : null;
  const sqlitePlayback = sqliteTrackLocator !== null;
  const version = catalog.albums;
  // A cold catalog load is not an index update and cannot hold the player
  // artwork hostage. The existing source-scoped IPC still validates ownership,
  // current configuration and the target at every filesystem boundary.
  const confirmedCheck = catalog.manualCheckPlayback === true && catalog.manualCheckArtwork === true;
  const blocked = !active || !available || (status.is_updating && !confirmedCheck) || (catalog.manualCheckPlayback === true && !confirmedCheck) || catalog.saveBlocked
    || catalog.mpdBusy || !!catalog.error || (catalog.loaded && catalog.loading);
  const [resolved, setResolved] = useState<{ key: string; version: readonly LocalAlbum[]; thumb: string } | null>(null);
  const [resolvedAlbum, setResolvedAlbum] = useState<{ key: string; version: readonly LocalAlbum[]; album: LocalAlbum } | null>(null);
  useLayoutEffect(() => {
    if (blocked || (!directory && !sqliteTrackLocator) || (sqliteTrackLocator && !catalog.loaded) || (status.thumb && !sqlitePlayback) || status.plex_image) return;
    let current = true;
    let unsubscribe = () => {};
    const publish = (thumb: string) => { if (current) setResolved({ key, version, thumb }); };
    let targetAlbum = album;
    void (async () => {
      if (!targetAlbum && sqliteTrackLocator) {
        const albumId = await sqliteLibrary.albumForTrack(sqliteTrackLocator);
        if (!current) return;
        targetAlbum = albumId ? catalog.albums.find(candidate => candidate.id === albumId) ?? null : null;
        if (targetAlbum) setResolvedAlbum({ key, version, album: targetAlbum });
      }
      if (targetAlbum && !catalog.albums.includes(targetAlbum)) return;
      // Use the same source-scoped album resolver/cache as Local cards. The
      // backend maps the opaque queue identity to an exact catalog membership;
      // it never infers by title, artist or path prefix.
      if (targetAlbum) {
        unsubscribe = localArtworkResolver.subscribe(targetAlbum.id, publish);
        const cover = await localArtworkResolver.resolve(targetAlbum, false, () => blocked || !current, undefined, () => current, false, sqlitePlayback);
        if (!current) return;
        if (cover) publish(cover);
        else if (onlineEnabled) {
          const cached = await audioService.getCachedOnlineAlbumCover(targetAlbum);
          if (cached && current) publish(cached);
        }
      } else if (directory && !sqliteTrackLocator) {
        // Existing Local behavior includes the physical folder and CUE folder
        // fallback when no unambiguous catalog album is available.
        let cover: string | null = null;
        // A virtual CUE directory retains folder-cover priority before its
        // embedded picture, just like the existing album/disc resolver.
        const paths = /\.cue$/i.test(directory)
          ? [directory.slice(0, directory.lastIndexOf("/")), directory] : [directory];
        for (const path of paths) {
          if (!current) return;
          cover = await audioService.getPlayerLocalCover(path);
          if (cover) break;
        }
        if (!current) return;
        if (cover) publish(cover);
        else if (onlineEnabled && !catalog.loaded) ensureCatalog();
      }
    })().catch(error => { if (current) logError("artwork.album-load", error); });
    return () => { current = false; unsubscribe(); };
  }, [key, directory, album, sqliteTrackLocator, sqlitePlayback, version, blocked, onlineEnabled, status.thumb, status.plex_image, catalog.loaded, ensureCatalog]);
  // Blocking suspends new reads, not an image already resolved for this media/catalog.
  if (status.plex_image || (status.thumb && !sqlitePlayback)) return status;
  // Existing saved browser rows may supply a preview on the first run after
  // upgrading too. They never become a confirmed album or authorize an IPC.
  const mappedAlbum = resolvedAlbum?.key === key && resolvedAlbum.version === version ? resolvedAlbum.album : null;
  const previewAlbum = album ?? mappedAlbum ?? localAlbumForMedia(status.current_media, catalog.presentationAlbums ?? []);
  const preview = localPlaybackThumbnail.get(status.current_media)
    ?? (previewAlbum ? localAlbumThumbnails.get(previewAlbum) : null);
  const resolvedCover = resolved?.key === key && resolved.version === version ? resolved.thumb : null;
  // Queue items may carry the persisted small thumbnail. Keep it as an instant
  // placeholder, then prefer the full source-scoped SQLite album artwork.
  const thumb = sqlitePlayback ? resolvedCover ?? status.thumb ?? preview : resolvedCover ?? preview;
  return thumb ? { ...status, thumb } : status;
}
