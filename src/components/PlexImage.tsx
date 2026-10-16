import React, { useEffect, useLayoutEffect, useRef, useState } from "react";

import { plexService } from "../services/plex";
import {
  PlexArtworkCache,
  PlexImageVisibilityGate,
  plexImageCacheKey,
  type PlexArtworkLease,
} from "../services/plexArtworkCache";
import type { PlexImageRef } from "../types/plex";

export const PLEX_IMAGE_ROOT_MARGIN = "400px 0px";

const artworkCache = new PlexArtworkCache(
  (image) => plexService.getImage(image),
  (bytes) => URL.createObjectURL(new Blob([bytes])),
  (url) => URL.revokeObjectURL(url),
);

interface PlexImageProps
  extends Omit<React.ImgHTMLAttributes<HTMLImageElement>, "src" | "onError"> {
  image: PlexImageRef;
  cacheScope?: object;
  fallback?: React.ReactNode;
  onLoadError?: () => void;
}

export const PlexImage: React.FC<PlexImageProps> = ({
  image,
  cacheScope,
  fallback = null,
  onLoadError,
  ...imgProps
}) => {
  const containerRef = useRef<HTMLSpanElement>(null);
  const visibilityGateRef = useRef(new PlexImageVisibilityGate());
  const leaseRef = useRef<PlexArtworkLease | null>(null);
  const [objectUrl, setObjectUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [visibleImageKey, setVisibleImageKey] = useState<string | null>(null);
  const onLoadErrorRef = useRef(onLoadError);
  onLoadErrorRef.current = onLoadError;
  const imageKey = plexImageCacheKey(image, cacheScope);

  useLayoutEffect(() => {
    visibilityGateRef.current.reset(image, cacheScope);
    setObjectUrl(null);
    setFailed(false);

    // Pin an already loaded image and present it before paint, without waiting for intersection.
    const cached = artworkCache.acquireCached(image, cacheScope);
    if (cached) {
      leaseRef.current = cached;
      setObjectUrl(cached.objectUrl);
      return () => {
        cached.release();
        if (leaseRef.current === cached) leaseRef.current = null;
      };
    }

    const node = containerRef.current;
    if (!node || !("IntersectionObserver" in window)) {
      if (visibilityGateRef.current.markVisible(image, cacheScope)) {
        setVisibleImageKey(imageKey);
      }
      return;
    }

    const observer = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((entry) => entry.isIntersecting) &&
          visibilityGateRef.current.markVisible(image, cacheScope)
        ) {
          setVisibleImageKey(imageKey);
          observer.disconnect();
        }
      },
      { rootMargin: PLEX_IMAGE_ROOT_MARGIN },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [imageKey]);

  useEffect(() => {
    if (
      visibleImageKey !== imageKey ||
      !visibilityGateRef.current.canAcquire(image, cacheScope)
    ) {
      return;
    }

    let active = true;
    const lease = artworkCache.acquire(image, cacheScope);
    leaseRef.current = lease;
    void lease.promise.then(
      (url) => {
        if (active && visibilityGateRef.current.canAcquire(image, cacheScope)) setObjectUrl(url);
      },
      () => {
        if (active && visibilityGateRef.current.canAcquire(image, cacheScope)) {
          setFailed(true);
          onLoadErrorRef.current?.();
        }
      },
    );
    return () => {
      active = false;
      lease.release();
      if (leaseRef.current === lease) leaseRef.current = null;
    };
  }, [imageKey, visibleImageKey]);

  return (
    <span ref={containerRef} className="flex h-full w-full items-center justify-center">
      {!objectUrl || failed ? (
        fallback
      ) : (
        <img
          {...imgProps}
          src={objectUrl}
          onError={() => {
            const lease = leaseRef.current;
            leaseRef.current = null;
            lease?.invalidate();
            setObjectUrl(null);
            setFailed(true);
            onLoadErrorRef.current?.();
          }}
        />
      )}
    </span>
  );
};
