import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { PlaybackStatus, QueueArtworkContext, TrackMetadata } from "../types/audio";
import { localAlbumCatalog } from "./localAlbumCatalogSession";
import { localArtworkResolver } from "./localArtworkSession";
import { artworkReadBlocked } from "./localArtwork";
import { queueItemArtwork } from "./nowPlayingPresentation";
import { localAlbumForMedia } from "./playerNavigation";
import { VisibleQueueArtwork } from "./visibleQueueArtwork";
import { audioService } from "../services/audio";
import { logError } from "./frontendDiagnostics";

const localMedia = (item: TrackMetadata) => item.media_locator?.kind === "plex" ? null
  : { kind: "local" as const, uri: item.media_locator?.kind === "local" ? item.media_locator.uri : item.uri ?? "", queue_index: 0 };

export function useLocalQueueArtwork(items: TrackMetadata[], start: number, end: number,
  open: boolean, available: boolean, status: PlaybackStatus | undefined,
  context: QueueArtworkContext | undefined, detached: boolean) {
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const blocked = !open || !available || !context || context.blocked || !!status?.is_updating;
  const visible = items.slice(start, end).map((item, offset) => queueItemArtwork(item, start + offset, status));
  const hasLocal = visible.some(item => !item.thumb && !item.plex_image && !!localMedia(item)?.uri);
  const revision = context?.generation;
  const detachedRevision = useRef<number | undefined>(undefined);
  const detachedBlocked = useRef(true);
  useEffect(() => {
    if (!detached || !context) return;
    const changed = detachedRevision.current !== undefined && detachedRevision.current !== revision;
    const wasBlocked = detachedBlocked.current;
    detachedRevision.current = revision;
    detachedBlocked.current = blocked;
    if (!hasLocal && !localAlbumCatalog.getSnapshot().loaded) return;
    localAlbumCatalog.setUpdating(blocked);
    if (!blocked && changed && !wasBlocked && localAlbumCatalog.getSnapshot().loaded) localAlbumCatalog.requestRefresh();
  }, [detached, revision, blocked, hasLocal, !!context]);
  useEffect(() => {
    if (!blocked && hasLocal) localAlbumCatalog.ensureLoaded(false, true);
  }, [blocked, hasLocal, catalog.loaded, catalog.loading, catalog.mpdBusy, catalog.saveBlocked]);
  const readBlocked = blocked || artworkReadBlocked(!!status?.is_updating, catalog)
    || (detached && (detachedRevision.current !== revision || (detachedBlocked.current && !blocked)));
  useLayoutEffect(() => {
    if (detached) localArtworkResolver.setContext(readBlocked, catalog.albums);
  }, [detached, readBlocked, catalog.albums]);

  // Logical identity only: polling metadata/elapsed changes do not restart reads.
  const queueKey = JSON.stringify(items.map(item => item.media_locator?.kind === "plex"
    ? ["plex", item.media_locator.server_id, item.media_locator.part_key] : ["local", localMedia(item)?.uri]));
  const session = useMemo(() => readBlocked ? null : {}, [readBlocked, queueKey, catalog.albums, revision, context?.onlineEnabled]);
  const [, redraw] = useState(0);
  const online = useRef(false);
  online.current = context?.onlineEnabled ?? false;
  const controller = useMemo(() => new VisibleQueueArtwork(async (album, consumerActive) => {
    const active = () => consumerActive()
      && !artworkReadBlocked(false, localAlbumCatalog.getSnapshot())
      && localAlbumCatalog.getSnapshot().albums.includes(album);
    try {
      if (!active()) return null;
      const cover = await localArtworkResolver.resolve(album, false, false, undefined, active);
      if (!active()) return null;
      if (cover || !online.current) return cover;
      const cached = await audioService.getCachedOnlineAlbumCover(album);
      return active() ? cached : null;
    } catch (error) {
      if (active()) logError("artwork.album-load", error);
      return null;
    }
  }, (album, publish) => localArtworkResolver.subscribe(album.id, publish), () => redraw(value => value + 1)), []);
  useLayoutEffect(() => {
    controller.begin(session);
    return () => controller.begin(null);
  }, [controller, session]);
  const albums = visible.filter(item => !item.thumb && !item.plex_image)
    .map(item => localAlbumForMedia(localMedia(item), catalog.albums)).filter(album => album !== null);
  const visibleKey = JSON.stringify(albums.map(album => album.id));
  useEffect(() => { controller.show(albums); }, [controller, session, visibleKey]);
  return (item: TrackMetadata): TrackMetadata => {
    if (readBlocked || item.thumb || item.plex_image || item.media_locator?.kind === "plex") return item;
    const cover = controller.get(session, localAlbumForMedia(localMedia(item), catalog.albums));
    return cover ? { ...item, thumb: cover } : item;
  };
}
