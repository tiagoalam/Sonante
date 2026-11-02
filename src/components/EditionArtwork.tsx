import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { Disc3 } from 'lucide-react';
import type { EditionEntry } from '../utils/albumEditions';
import { editionArtworkCandidates } from '../utils/albumEditionArtwork';
import { PlexImage } from './PlexImage';
import { localArtworkResolver, localAlbumThumbnails } from '../utils/localArtworkSession';
import { localAlbumCatalog } from '../utils/localAlbumCatalogSession';
import { artworkReadBlocked } from '../utils/localArtwork';
import { navidromeAlbumCover } from '../utils/navidromeArtwork';

// Reuse catalog cover caches across list/detail. Images never expand albums or
// fetch tracks; failing sources fall back only within this measured edition.
export function EditionArtwork({ entry, copies, active, updating, coverId }: { entry: EditionEntry; copies?: readonly EditionEntry[]; active: boolean; updating: boolean; coverId?: string | null }) {
  const element = useRef<HTMLDivElement>(null), [near, setNear] = useState(false);
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const blocked = artworkReadBlocked(updating, catalog);
  const candidates = useMemo(() => editionArtworkCandidates(entry, copies, coverId), [entry, copies, coverId]);
  const keys = JSON.stringify(candidates.map(candidate => candidate.key));
  const context = useMemo(() => ({}), [keys, blocked, catalog.albums]);
  const [attempt, setAttempt] = useState<{ context: object; index: number } | null>(null);
  const [resolved, setResolved] = useState<{ context: object; index: number; url: string } | null>(null);
  const index = attempt?.context === context ? attempt.index : 0, candidate = candidates[index];
  const live = useRef({ context, active }); live.current = { context, active };
  const advance = () => {
    if (!live.current.active || live.current.context !== context) return;
    setAttempt(previous => ({ context, index: Math.max(index + 1, previous?.context === context ? previous.index : 0) }));
  };
  useEffect(() => {
    const node = element.current; if (!node) return;
    if (typeof IntersectionObserver === 'undefined') { setNear(true); return; }
    const observer = new IntersectionObserver(rows => { if (rows.some(row => row.isIntersecting)) { setNear(true); observer.disconnect(); } }, { rootMargin: '150px' });
    observer.observe(node); return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!active || !near || !candidate || candidate.kind === 'plex') return;
    let current = true;
    const valid = () => current && live.current.active && live.current.context === context;
    void (async () => {
      let image: string | null = null;
      if (candidate.kind === 'local') {
        const album = catalog.albums.find(album => album.id === candidate.album.id);
        image = localArtworkResolver.get(candidate.album.id);
        if (!image) {
          // Saved thumbnails can paint immediately, but never authorize a read.
          const thumbnail = localAlbumThumbnails.get(album ?? candidate.album);
          if (thumbnail && valid()) { image = thumbnail; setResolved({ context, index, url: thumbnail }); }
          if (album && !blocked) image = await localArtworkResolver.resolve(album, false, () => !valid() || blocked, undefined, valid) ?? image;
        }
      } else image = await navidromeAlbumCover(candidate.connectionId, candidate.coverId, valid);
      if (!valid()) return;
      if (image) setResolved({ context, index, url: image }); else advance();
    })().catch(() => { if (valid()) advance(); });
    return () => { current = false; };
  }, [context, index, near, active]);
  const image = resolved?.context === context && resolved.index === index ? resolved.url : null;
  const fallback = <Disc3 aria-hidden="true" className="h-1/3 w-1/3 text-[color:var(--theme-fg-faint,rgb(255_255_255_/_0.2))]" />;
  return <div ref={element} className="flex h-full w-full items-center justify-center bg-[var(--theme-surface-subtle,rgb(255_255_255_/_0.05))]">
    {active && candidate?.kind === 'plex' ? <PlexImage key={candidate.key} image={candidate.image} alt="" className="h-full w-full object-cover" fallback={fallback} onLoadError={advance} />
      : image ? <img src={image} alt="" loading="lazy" decoding="async" className="h-full w-full object-cover" onError={advance} /> : fallback}
  </div>;
}
