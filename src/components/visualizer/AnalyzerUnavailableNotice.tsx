import React from "react";
import { CircleHelp, Disc3 } from "lucide-react";
import type { TrackMetadata } from "../../types/audio";
import { TrackArtwork } from "../TrackArtwork";

interface Props {
  label: string;
  help?: string;
  artworkTrack?: Pick<TrackMetadata, "thumb" | "plex_image">;
  className?: string;
}

/** A quiet, static fallback. Artwork is genuine album art, never fabricated signal data. */
export const AnalyzerUnavailableNotice: React.FC<Props> = ({ label, help, artworkTrack, className = "" }) => {
  const hasArtwork = Boolean(artworkTrack?.thumb || artworkTrack?.plex_image);
  return <div role="status" aria-live="polite" data-visualizer-inactive
    className={`relative isolate mx-auto flex max-h-full w-full max-w-xl items-center overflow-hidden rounded-xl border border-white/10 shadow-lg ${className}`}>
    {hasArtwork ? <div data-visualizer-static-art aria-hidden="true"
      className="absolute inset-0 opacity-20 saturate-75 blur-[1px]">
      <TrackArtwork track={artworkTrack!} />
    </div> : <div data-visualizer-still-mark aria-hidden="true"
      className="absolute inset-0 bg-[radial-gradient(ellipse_at_30%_45%,rgba(229,160,13,0.16),transparent_42%),linear-gradient(115deg,rgba(255,255,255,0.035),transparent_55%)]" />}
    <div className="relative z-10 flex min-h-[64px] w-full items-center gap-3 bg-black/55 px-4 py-3 text-left backdrop-blur-[2px]">
      <span aria-hidden="true" className="grid h-9 w-9 shrink-0 place-items-center rounded-full border border-white/10 bg-black/25 text-white/60">
        {hasArtwork ? <CircleHelp size={18} /> : <Disc3 size={18} />}
      </span>
      <div className="min-w-0">
        <p className="text-sm font-semibold leading-snug text-white/90">{label}</p>
        {help && <p className="mt-1 text-xs leading-relaxed text-white/65">{help}</p>}
      </div>
    </div>
  </div>;
};
