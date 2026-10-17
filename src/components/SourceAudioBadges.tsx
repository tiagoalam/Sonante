import React, { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SourceAudioFacts } from "../types/sourceAudio";
import { sourceAudioBadge, albumAudioBadge } from "../utils/sourceAudio";

// Static full class names keep the palette stable in every host/build.
export const sourceBadgeColors = {
  lossless: "border-emerald-300/30 bg-emerald-950 text-emerald-200",
  lossy: "border-sky-300/30 bg-sky-950 text-sky-200",
  pcm: "border-violet-300/30 bg-violet-950 text-violet-200",
  dsd: "border-rose-300/30 bg-rose-950 text-rose-200",
  other: "border-slate-300/30 bg-slate-800 text-slate-200",
};
export const SourceAudioBadges: React.FC<{ info?: SourceAudioFacts | null; album?: SourceAudioFacts[]; compact?: boolean }> = ({ info: track, album, compact = false }) => {
  const { t } = useTranslation();
  const id = useId();
  const info = album?.[0] ?? track;
  const badge = album ? albumAudioBadge(album) : sourceAudioBadge(info);
  if (!badge || !info) return null;
  const facts = [badge.depth && info.sample_type === "integer" ? `${badge.depth} bit` : null,
    badge.rate ? `${badge.rate / 1000} kHz` : null].filter(Boolean).join(" · ");
  const label = badge.label === "mixed" ? t("sourceAudio.mixed") : badge.label;
  const help = `${t(`sourceAudio.${info.provenance}`)}${facts ? ` · ${facts}` : ""}. ${t(album ? badge.hiRes ? "sourceAudio.albumHiRes" : "sourceAudio.albumFormat" : badge.hiRes ? info.sample_type === "dsd" ? "sourceAudio.dsdHiRes" : "sourceAudio.pcmHiRes" : "sourceAudio.formatOnly")} ${t("sourceAudio.limit")}`;
  const base = `rounded border font-semibold leading-none tracking-wide ${compact ? "px-1 py-0.5 text-[9px]" : "px-2 py-1 text-[11px]"}`;
  return <span className="relative mt-1 inline-flex w-fit max-w-full flex-wrap items-center gap-1 outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] group/source" tabIndex={compact ? undefined : 0} aria-describedby={compact ? undefined : id} aria-label={`${label}${badge.hiRes ? `, ${t("sourceAudio.hiResLabel")}` : ""}`} title={help} data-source-audio={info.provenance}>
    <span className={`${base} ${sourceBadgeColors[badge.family]}`}>{label}</span>
    {badge.hiRes && <span className={`${base} border-amber-300/40 bg-amber-950 text-amber-200`}>{t("sourceAudio.hiResLabel")}</span>}
    {!compact && <span id={id} role="tooltip" className="pointer-events-none absolute bottom-full left-0 z-50 mb-2 hidden w-64 rounded border border-white/15 bg-[#161616] p-2 text-left text-xs font-normal leading-normal text-[#DDD4C4] shadow-lg group-hover/source:block group-focus-visible/source:block">{help}</span>}
  </span>;
};
