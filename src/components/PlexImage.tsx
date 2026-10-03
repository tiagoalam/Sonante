import React, { useEffect, useRef, useState } from "react";

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
  fallback?: React.ReactNode;
  onLoadError?: () => void;
}

export const PlexImage: React.FC<PlexImageProps> = ({
  image,
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
  const imageKey = plexImageCacheKey(image);

  useEffect(() => {
    visibilityGateRef.current.reset(image);
    setObjectUrl(null);
    setFailed(false);

    const node = containerRef.current;
    if (!node || !("IntersectionObserver" in window)) {
      if (visibilityGateRef.current.markVisible(image)) {
        setVisibleImageKey(imageKey);
      }
      return;
    }

    const observer = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((entry) => entry.isIntersecting) &&
          visibilityGateRef.current.markVisible(image)
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
      !visibilityGateRef.current.canAcquire(image)
    ) {
      return;
    }

    let active = true;
    const lease = artworkCache.acquire(image);
    leaseRef.current = lease;
    void lease.promise.then(
      (url) => {
        if (active) setObjectUrl(url);
      },
      () => {
        if (active) {
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
