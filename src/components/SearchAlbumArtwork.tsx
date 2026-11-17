import { useEffect, useRef, useState } from "react";
import { Disc3 } from "lucide-react";
import type { SearchRow } from "../utils/unifiedSearch";
import { PlexImage } from "./PlexImage";
import { Artwork as NavidromeArtwork } from "./NavidromeView";
import { localArtworkResolver, localAlbumThumbnails } from "../utils/localArtworkSession";
import { logError } from "../utils/frontendDiagnostics";

type AlbumRow = Extract<SearchRow, { kind: "album" }>;

const frame = "flex h-11 w-11 shrink-0 items-center justify-center overflow-hidden rounded-lg bg-[var(--theme-surface-accent,#332b15)] text-[color:var(--theme-fg-accent,#E5A00D)]";
const image = "h-full w-full object-cover";

function Placeholder() {
  return <Disc3 size={22} aria-hidden="true" />;
}

export function LocalSearchAlbumArtwork({ album, active, localReady, sqliteBacked, onlineArtworkEnabled, isLibraryUpdating }: {
  album: Extract<AlbumRow, { source: "local" }>['album'];
  active: boolean;
  localReady: boolean;
  sqliteBacked: boolean;
  onlineArtworkEnabled: boolean;
  isLibraryUpdating: boolean;
}) {
  const initialCover = localAlbumThumbnails.get(album) ?? localArtworkResolver.get(album.id);
  const [state, setState] = useState<{ album: typeof album; cover: string | null; failed: boolean }>(() => ({ album, cover: initialCover, failed: false }));
  const container = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const activeRef = useRef(active), readyRef = useRef(localReady), visibleRef = useRef(visible);
  const updatingRef = useRef(isLibraryUpdating);
  activeRef.current = active; readyRef.current = localReady; visibleRef.current = visible; updatingRef.current = isLibraryUpdating;
  const cover = state.album === album ? state.cover : initialCover;
  const failed = state.album === album && state.failed;

  useEffect(() => {
    setVisible(false);
    if (!active || !localReady) return;
    const node = container.current;
    if (!node || typeof IntersectionObserver === "undefined") { setVisible(true); return; }
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) { setVisible(true); observer.disconnect(); }
    }, { rootMargin: "150px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, [active, localReady, album]);

  useEffect(() => {
    let live = true;
    const unsubscribe = localArtworkResolver.subscribe(album.id, next => {
      if (live) setState({ album, cover: next, failed: false });
    });
    const preview = localAlbumThumbnails.get(album);
    if (preview) localArtworkResolver.seedPersistedLocal(album.id, preview);
    setState({ album, cover: preview ?? localArtworkResolver.get(album.id), failed: false });
    return () => { live = false; unsubscribe(); };
  }, [album]);

  useEffect(() => {
    if (!active || !localReady || !visible || cover || failed) return;
    let live = true;
    const current = () => live && activeRef.current && readyRef.current && visibleRef.current;
    void localArtworkResolver.resolve(album, sqliteBacked ? false : onlineArtworkEnabled,
      () => updatingRef.current, undefined, current)
      .then(found => { if (live && found) setState({ album, cover: found, failed: false }); })
      .catch(error => { if (live) logError("artwork.search-album", error); });
    return () => { live = false; };
  }, [album, active, localReady, visible, cover, failed, sqliteBacked, onlineArtworkEnabled, isLibraryUpdating]);

  return <span ref={container} className={frame}>
    {visible && cover && !failed
      ? <img src={cover} alt="" className={image} onError={() => setState({ album, cover: null, failed: true })} />
      : <Placeholder />}
  </span>;
}

export function SearchAlbumArtwork({ row, active, localReady, sqliteBacked, onlineArtworkEnabled, isLibraryUpdating, plexIdentity }: {
  row: AlbumRow;
  active: boolean;
  localReady: boolean;
  sqliteBacked: boolean;
  onlineArtworkEnabled: boolean;
  isLibraryUpdating: boolean;
  plexIdentity: object;
}) {
  if (row.source === "local") return <LocalSearchAlbumArtwork album={row.album} active={active} localReady={localReady}
    sqliteBacked={sqliteBacked} onlineArtworkEnabled={onlineArtworkEnabled} isLibraryUpdating={isLibraryUpdating} />;
  if (row.source === "plex") return <span className={frame}>
    {row.album.thumb
      ? <PlexImage image={row.album.thumb} cacheScope={plexIdentity} alt="" className={image} fallback={<Placeholder />} />
      : <Placeholder />}
  </span>;
  const connection = row.connection.connection_id;
  return <span className={frame}>
    {connection && row.album.cover_id
      ? <NavidromeArtwork connection={connection} id={row.album.cover_id} />
      : <Placeholder />}
  </span>;
}

