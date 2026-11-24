import { useEffect, useRef, useState } from "react";
import { Database, Folder, Server } from "lucide-react";
import { sqliteLibrary, type PersonalCollectionTarget, type PersonalCollectionArtwork as ArtworkMetadata } from "../services/sqliteLibrary";
import { localAlbumCatalog } from "../utils/localAlbumCatalogSession";
import { localArtworkResolver } from "../utils/localArtworkSession";
import { createCollectionArtworkFallback } from "../utils/collectionArtworkFallback.mjs";
import { PlexImage } from "./PlexImage";
import { Artwork as SubsonicArtwork } from "./NavidromeView";
import { resolvePersonalCollectionArtwork, type CollectionCover } from "../utils/personalCollectionArtwork.mjs";
// Opaque, bounded scope objects isolate Plex artwork across connection generations.
const plexScopes = new Map<string, object>();
function plexScope(key: string): object {
  let scope = plexScopes.get(key);
  if (!scope) { if (plexScopes.size >= 128) plexScopes.delete(plexScopes.keys().next().value!); scope = {}; plexScopes.set(key, scope); }
  return scope;
}
const localFallback = createCollectionArtworkFallback(sqliteLibrary.albumCover);

export function PersonalCollectionArtwork({ target, artwork, source, preferFullArtwork = false }: { target: PersonalCollectionTarget | null; artwork: ArtworkMetadata | null; source: "local" | "plex" | "subsonic"; preferFullArtwork?: boolean }) {
  const [cover, setCover] = useState<CollectionCover | null>(null);
  const key = JSON.stringify([target, artwork]);
  const ref = useRef<HTMLSpanElement>(null);
  const [near, setNear] = useState(false);
  useEffect(() => {
    const node = ref.current;
    if (!node || typeof IntersectionObserver === "undefined") { setNear(true); return; }
    const observer = new IntersectionObserver(entries => { setNear(entries.some(entry => entry.isIntersecting)) }, { rootMargin: "200px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let live = true;
    setCover(null);
    if (!target || !near) return () => { live = false; };
    void resolvePersonalCollectionArtwork(target, {
      local: async (path, current) => {
        const snapshot = localAlbumCatalog.getSnapshot();
        const album = snapshot.albums.find(item => path === `sqlite-album://${item.id}`);
        if (!album) return localFallback(path, current);
        return localArtworkResolver.resolve(album, false, () => localAlbumCatalog.getSnapshot().loading || Boolean(localAlbumCatalog.getSnapshot().error), undefined, current, false, preferFullArtwork);
      },
      artwork,
    }, () => live).then(result => { if (live) setCover(result); });
    return () => { live = false; };
  }, [key, near, preferFullArtwork]);
  const Icon = source === "plex" ? Server : source === "subsonic" ? Database : Folder;
  const fallback = <Icon size={34} />;
  return <span ref={ref} className="flex h-full w-full items-center justify-center overflow-hidden rounded-lg">
    {!cover ? fallback : cover.kind === "local" ? <img alt="" src={cover.src} className="h-full w-full object-cover" onError={() => setCover(null)} /> : cover.kind === "plex" ? <PlexImage image={cover.image} cacheScope={plexScope(cover.scope)} alt="" className="h-full w-full object-cover" fallback={fallback} /> : <SubsonicArtwork connection={cover.connection} id={cover.id} />}
  </span>;
}
