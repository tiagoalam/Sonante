import type { TrackMetadata } from "../types/audio";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";
import { localAlbumCatalog } from "./localAlbumCatalogSession";
import { localAlbumForMedia } from "./playerNavigation";
import { flattenAlbumDiscs } from "./localAlbumDiscs";
import { localQueueTrack, plexQueueTrack } from "./queueActions";

// Called only by an explicit whole-album menu action. Never infer album identity
// from title/artist or scan physical folders. Unknown identity refuses the action.
export async function loadQueuedAlbum(track: TrackMetadata, queueIndex?: number): Promise<TrackMetadata[]> {
  const locator = track.media_locator;
  if (locator?.kind === "plex" && queueIndex !== undefined) {
    const { navigation } = await plexService.resolvePlayerNavigation({ kind: "plex", server_id: locator.server_id, part_key: locator.part_key, queue_index: queueIndex });
    return (await plexService.getAlbumTracks(navigation.album.rating_key)).map(item => plexQueueTrack(item, navigation.album));
  }
  const uri = locator?.kind === "local" ? locator.uri : !locator ? track.uri : undefined;
  if (!uri) throw "QUEUE_INSERT_ALBUM_UNAVAILABLE";
  const snapshot = localAlbumCatalog.getSnapshot();
  const albums = snapshot.loaded ? snapshot.albums : await audioService.getLocalAlbums();
  const album = localAlbumForMedia({ kind: "local", uri, queue_index: 0 }, albums);
  if (!album) throw "QUEUE_INSERT_ALBUM_UNAVAILABLE";
  return flattenAlbumDiscs(await audioService.getLocalAlbumTracks(album.id)).tracks.map(item => localQueueTrack(item, album));
}
