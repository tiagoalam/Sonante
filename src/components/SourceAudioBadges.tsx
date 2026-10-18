import React, { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SourceAudioFacts } from "../types/sourceAudio";
import { sourceAudioBadge, albumAudioBadge } from "../utils/sourceAudio";

// Static full class names keep the palette stable in every host/build.
export const sourceBadgeColors = {
  lossless: "border-[color:var(--theme-border-success,var(--color-emerald-300))]/30 bg-[var(--theme-surface-success,var(--color-emerald-950))] text-[color:var(--theme-fg-success,var(--color-emerald-200))]",
  lossy: "border-[color:var(--theme-border-info,var(--color-sky-300))]/30 bg-[var(--theme-surface-info,var(--color-sky-950))] text-[color:var(--theme-fg-info,var(--color-sky-200))]",
  pcm: "border-[color:var(--theme-border-purple,var(--color-violet-300))]/30 bg-[var(--theme-surface-purple,var(--color-violet-950))] text-[color:var(--theme-fg-purple,var(--color-violet-200))]",
  dsd: "border-[color:var(--theme-border-error,var(--color-rose-300))]/30 bg-[var(--theme-surface-error,var(--color-rose-950))] text-[color:var(--theme-fg-error,var(--color-rose-200))]",
  other: "border-[color:var(--theme-border-soft,var(--color-slate-300))]/30 bg-[var(--theme-surface-raised,var(--color-slate-800))] text-[color:var(--theme-fg-muted,var(--color-slate-200))]",
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
  return <span className="relative mt-1 inline-flex w-fit max-w-full flex-wrap items-center gap-1 outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] group/source" tabIndex={compact ? undefined : 0} aria-describedby={compact ? undefined : id} aria-label={`${label}${badge.hiRes ? `, ${t("sourceAudio.hiResLabel")}` : ""}`} title={help} data-source-audio={info.provenance}>
    <span className={`${base} ${sourceBadgeColors[badge.family]}`}>{label}</span>
    {badge.hiRes && <span className={`${base} border-[color:var(--theme-border-accent,var(--color-amber-300))]/40 bg-[var(--theme-surface-accent,var(--color-amber-950))] text-[color:var(--theme-fg-accent,var(--color-amber-200))]`}>{t("sourceAudio.hiResLabel")}</span>}
    {!compact && <span id={id} role="tooltip" className="pointer-events-none absolute bottom-full left-0 z-50 mb-2 hidden w-64 rounded border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.15))] bg-[var(--theme-surface-base,#161616)] p-2 text-left text-xs font-normal leading-normal text-[color:var(--theme-fg-accent,#DDD4C4)] shadow-lg group-hover/source:block group-focus-visible/source:block">{help}</span>}
  </span>;
};
