import type { LocalAlbum } from "../types/local";

type CoverLookup = (album: LocalAlbum) => Promise<string | null>;
type FolderLookup = (path: string) => Promise<string | null>;
type CurrentFlag = boolean | (() => boolean);
const current = (flag: CurrentFlag): boolean => typeof flag === "function" ? flag() : flag;

const onlineInFlight = new Map<string, Promise<string | null>>();

export async function lookupAlbumArtwork(
  album: LocalAlbum,
  getLocalCover: FolderLookup,
  getOnlineCover: CoverLookup,
  onlineEnabled: CurrentFlag,
  libraryUpdating: CurrentFlag,
): Promise<string | null> {
  const paths = new Set([album.folder_path, ...album.discs.map((disc) => disc.folder_path)]);
  for (const path of paths) {
    const cover = await getLocalCover(path);
    if (cover) return cover;
  }
  if (!current(onlineEnabled) || current(libraryUpdating)) return null;

  let pending = onlineInFlight.get(album.id);
  if (!pending) {
    pending = getOnlineCover(album).finally(() => onlineInFlight.delete(album.id));
    onlineInFlight.set(album.id, pending);
  }
  return pending;
}
