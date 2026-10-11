import React from "react";
import { ArrowRight, AudioLines, CircleHelp, Cpu, Music2, Speaker, Volume2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";
import { signalPathInformation } from "../utils/signalPath";

interface Props {
  route: ConfiguredSignalRoute | null;
  status: PlaybackStatus;
  health: MpdHealth;
  compact?: boolean;
}

// A read-only view of confirmed configuration, never a Settings draft.
export const SignalPath: React.FC<Props> = ({ route, status, health, compact = false }) => {
  const { t } = useTranslation();
  const information = signalPathInformation(status, health);
  const format = information.format;
  const formatLabel = format.kind === "pcm"
    ? t("signalPath.pcm", { rate: format.rateKhz, bits: format.bits, channels: format.channels })
    : t(format.kind === "unknown" ? "signalPath.unknownFormat" : "signalPath.noSignal");
  const mode = route ? t(`signalPath.${route.mode}`) : t("signalPath.unknownRoute");
  const source = information.source ? t(`player.source.${information.source}`) : t("signalPath.unknownSource");
  const state = t(`signalPath.${information.state}`);

  if (compact) {
    // No stale format or inferred signal. Full MPD report remains in help.
    const secondary = information.state === "unavailable" ? state
      : information.state === "paused" || information.state === "stopped"
        ? [information.source ? source : null, state].filter(Boolean).join(" · ")
        : format.kind === "pcm"
          ? t("signalPath.pcmCompact", { rate: format.rateKhz, bits: format.bits })
          : information.source ? source : "";
    const description = `${t("signalPath.title")}: ${source} · ${formatLabel} · ${mode} · ${state}`;
    return (
      <section tabIndex={0} aria-label={description}
        title={`${t("signalPath.help")} · ${description}`}
        aria-description={t("signalPath.help")}
        className="group/compact-signal relative flex min-w-0 flex-col gap-1 text-[10px] leading-tight text-white/55 focus-visible:outline focus-visible:outline-[#E5A00D]">
        <span className="truncate font-medium text-white/70" title={mode}>{mode}</span>
        {secondary ? <span className="truncate text-[9px]" title={description}>{secondary}</span> : null}
        <span role="tooltip" className="pointer-events-none absolute bottom-full right-0 z-50 mb-2 hidden w-64 rounded-lg border border-white/15 bg-[#202020] p-2 text-[11px] leading-relaxed text-white/80 shadow-lg group-hover/compact-signal:block group-focus-within/compact-signal:block">
          {t("signalPath.help")} · {description}
        </span>
      </section>
    );
  }

  const stages = [
    { label: "Sonante", icon: Music2 },
    { label: "MPD", icon: Cpu },
    { label: route?.mode === "shared" ? "ALSA default" : route?.mode === "direct" && route.endpoint ? "ALSA hw" : "ALSA", icon: AudioLines },
    ...(route?.mode === "shared" ? [{ label: t("signalPath.system"), icon: Volume2 }] : []),
    { label: route?.mode === "direct" ? "DAC" : t("signalPath.output"), icon: Speaker },
  ];

  return (
    <section aria-label={t("signalPath.title")} className="relative w-full min-w-0 rounded-xl border border-[#E5A00D]/20 bg-black/25 p-3">
      <div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-1">
        <h3 className="text-[11px] font-medium text-white/65">{t("signalPath.title")}</h3>
        <div className="flex items-center gap-2">
          <span className="text-[10px] text-white/50">{mode} · {state}</span>
          <div className="group/signal-help flex">
            <button type="button" aria-label={t("signalPath.help")} className="rounded text-white/45 hover:text-white/80 focus-visible:outline focus-visible:outline-[#E5A00D]">
              <CircleHelp size={14} aria-hidden="true" />
            </button>
            <span role="tooltip" className="pointer-events-none absolute inset-x-0 top-7 z-20 mt-1 hidden w-full rounded-lg border border-white/15 bg-[#202020] p-2 text-[11px] leading-relaxed text-white/80 shadow-lg group-hover/signal-help:block group-focus-within/signal-help:block">
              {t("signalPath.help")}
            </span>
          </div>
        </div>
      </div>
      {route && (
        <ol className={`mt-2 grid items-start gap-2 ${route.mode === "shared" ? "grid-cols-5" : "grid-cols-4"}`} aria-label={t("signalPath.configured")}>
          {stages.map(({ label, icon: Icon }, index) => (
            <li key={index} className="relative flex min-w-0 flex-col items-center gap-1 text-center">
              {index > 0 && <ArrowRight size={10} className="absolute -left-2 top-0.5 text-[#E5A00D]/45" aria-hidden="true" />}
              <Icon size={14} className="shrink-0 text-[#E5A00D]/75" aria-hidden="true" />
              <span className="w-full break-words text-[9px] leading-tight text-white/65">{label}</span>
            </li>
          ))}
        </ol>
      )}
      {route?.mode === "direct" && route.endpoint && <p className="mt-2 break-all text-[10px] text-white/50">{t("signalPath.endpointShort")}: {route.endpoint}</p>}
      <div className="mt-2 flex flex-wrap gap-x-2 gap-y-1 text-[10px] text-white/65">
        {information.source && <span>{source}</span>}
        <span title={t("signalPath.reported")}>{formatLabel}</span>
        {information.volumeBackend !== "unavailable" && <span>{t("signalPath.volume")}: {t(`player.volumeBackend.${information.volumeBackend}`)}</span>}
      </div>
      {/* Reserved presentation slot: future verified metadata, no inferred quality badges. */}
      <div data-signal-quality-slot />
    </section>
  );
};
