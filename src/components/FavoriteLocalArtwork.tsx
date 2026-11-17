import React, { useEffect, useRef, useState } from "react";
import type { FavoriteAlbum } from "../types/favorite";
import type { LocalAlbum } from "../types/local";
import { localAlbumThumbnails, localArtworkResolver } from "../utils/localArtworkSession";
import { logError } from "../utils/frontendDiagnostics";

interface Props {
  favorite: FavoriteAlbum;
  album: LocalAlbum | null;
  localAlbums: readonly LocalAlbum[];
  artworkReady: boolean;
  alt: string;
  className: string;
  fallback: React.ReactNode;
}

export const FavoriteLocalArtwork: React.FC<Props> = ({
  favorite, album, localAlbums, artworkReady, alt, className, fallback,
}) => {
  const rootRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const mounted = useRef(true);
  const [visible, setVisible] = useState(false);
  const [cover, setCover] = useState<string | null>(() => album
    ? localArtworkResolver.get(album.id) ?? localAlbumThumbnails.get(album) ?? favorite.thumb ?? null
    : favorite.thumb ?? null);
  const [failedCover, setFailedCover] = useState<string | null>(null);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    if (!album) return;
    if (typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const node = rootRef.current;
    if (!node) {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(entries => {
      if (entries[0]?.isIntersecting) {
        setVisible(true);
        observer.disconnect();
      }
    }, { rootMargin: "150px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, [album]);

  useEffect(() => {
    let active = true;
    setFailedCover(null);
    if (!album) {
      setCover(favorite.thumb ?? null);
      return;
    }

    const unsubscribe = localArtworkResolver.subscribe(album.id, value => {
      if (active) {
        setCover(value);
        setFailedCover(null);
      }
    });
    const memoryCover = localArtworkResolver.get(album.id);
    const persistedThumbnail = memoryCover ? null : localAlbumThumbnails.get(album);
    if (!memoryCover && persistedThumbnail) localArtworkResolver.seedPersistedLocal(album.id, persistedThumbnail);
    const cachedCover = memoryCover ?? persistedThumbnail;
    setCover(cachedCover ?? favorite.thumb ?? null);

    if (!visible || !artworkReady || cachedCover) {
      return () => { active = false; unsubscribe(); };
    }

    const current = () => active && mounted.current && artworkReady && localAlbums.includes(album);
    void localArtworkResolver.resolve(album, false, () => !current(), undefined, current)
      .then(found => {
        if (current()) setCover(found ?? favorite.thumb ?? null);
      })
      .catch(error => { if (current()) logError("artwork.album-load", error); });
    return () => { active = false; unsubscribe(); };
  }, [album, artworkReady, favorite.id, favorite.path_or_key, favorite.thumb, localAlbums, visible]);

  const captureCover = (image: HTMLImageElement, trigger: "on_load" | "already_complete") => {
    if (!album || !artworkReady || !visible || !cover || cover === failedCover
      || (image.currentSrc || image.src) !== cover) return;
    localAlbumThumbnails.capture(album, image,
      () => mounted.current && artworkReady && localAlbums.includes(album), trigger);
  };

  useEffect(() => {
    const image = imageRef.current;
    if (image?.complete) captureCover(image, "already_complete");
  }, [album, artworkReady, cover, failedCover, visible]);

  return (
    <div ref={rootRef} className="absolute inset-0">
      {cover && cover !== failedCover ? (
        <img ref={imageRef} src={cover} alt={alt} className={className} loading="lazy"
          onLoad={event => captureCover(event.currentTarget, "on_load")}
          onError={() => setFailedCover(cover)} />
      ) : fallback}
    </div>
  );
};
