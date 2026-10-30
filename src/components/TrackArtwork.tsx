import React, { useState } from "react";
import { Disc3 } from "lucide-react";
import type { TrackMetadata } from "../types/audio";
import { PlexImage } from "./PlexImage";

export const TrackArtwork: React.FC<{ track: Pick<TrackMetadata, "thumb" | "plex_image">; alt?: string; }> = ({ track, alt = "" }) => {
  const [failedSource, setFailedSource] = useState<string | null>(null);
  const fallback = <Disc3 aria-hidden="true" className="h-1/3 w-1/3 text-[color:var(--theme-fg-faint,rgb(255_255_255_/_0.2))]" />;
  return <div className="flex h-full w-full items-center justify-center bg-[var(--theme-surface-subtle,rgb(255_255_255_/_0.05))]">
    {track.plex_image ? <PlexImage image={track.plex_image} alt={alt} className="h-full w-full object-cover" fallback={fallback} />
      : track.thumb && failedSource !== track.thumb ? <img src={track.thumb} alt={alt} loading="lazy" decoding="async"
        className="h-full w-full object-cover" onError={() => setFailedSource(track.thumb ?? null)} /> : fallback}
  </div>;
};
