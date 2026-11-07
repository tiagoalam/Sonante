import React from "react";
import { ArrowRight, AudioLines, CircleHelp, Cpu, Music2, Speaker, Volume2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";
import { signalPathInformation, observedAlsaOutput, observedDsdTransport, requiresDeviceVolumeControl } from "../utils/signalPath";

interface Props {
  route: ConfiguredSignalRoute | null;
  status: PlaybackStatus;
  health: MpdHealth;
  compact?: boolean;
  horizontal?: boolean;
}

// Native dialog uses the browser top layer (also inside fullscreen/container
// layouts), traps focus and makes the background inert without global timers.
export function SignalPathInfo({ children, title, openLabel, closeLabel }: {
  children: React.ReactNode; title: string; openLabel: string; closeLabel: string;
}) {
  const [open, setOpen] = React.useState(false);
  const dialog = React.useRef<HTMLDialogElement>(null);
  const trigger = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => {
    if (!open || !dialog.current) return;
    const element = dialog.current;
    element.showModal();
    return () => { if (element.open) element.close(); trigger.current?.focus(); };
  }, [open]);
  return <>
    <button ref={trigger} type="button" data-signal-info aria-label={openLabel}
      aria-haspopup="dialog" aria-expanded={open} onKeyDown={event => { if (event.key === " " || event.key === "Enter") event.stopPropagation(); }} onClick={event => { event.stopPropagation(); setOpen(true); }}
      className="shrink-0 rounded-full p-1.5 text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.6))] hover:bg-[var(--theme-surface-subtle,rgb(255_255_255_/_0.1))] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)]">
      <CircleHelp size={18} aria-hidden="true" />
    </button>
    {open && <dialog ref={dialog} aria-label={title} aria-modal="true" tabIndex={-1}
      className="m-auto max-h-[85dvh] w-11/12 max-w-2xl overflow-y-auto rounded-xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.15))] bg-[var(--theme-surface-panel,#181818)] p-0 text-[color:var(--theme-fg-primary,rgb(255_255_255_/_0.85))] shadow-2xl backdrop:bg-black/75"
      onKeyDown={event => event.stopPropagation()}
      onCancel={event => { event.preventDefault(); setOpen(false); }} onClose={() => setOpen(false)}
      onClick={event => { event.stopPropagation(); if (event.target === event.currentTarget) setOpen(false); }}>
      <div className="p-5">
        <header className="mb-4 flex items-center justify-between gap-4">
          <h2 className="text-lg font-semibold">{title}</h2>
          <button type="button" autoFocus aria-label={closeLabel} onClick={() => setOpen(false)}
            className="rounded p-2 text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.7))] hover:bg-[var(--theme-surface-subtle,rgb(255_255_255_/_0.1))] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)]"><X size={20} /></button>
        </header>
        {children}
      </div>
    </dialog>}
  </>;
}

export const SignalPath: React.FC<Props> = ({ route, status, health, compact = false, horizontal = false }) => {
  const { t } = useTranslation();
  const information = signalPathInformation(status, health);
  const observedOutput = observedAlsaOutput(route, status, health);
  const transport = observedOutput && observedDsdTransport(status, observedOutput);
  const format = information.format;
  const formatLabel = format.kind === "pcm"
    ? t("signalPath.pcm", { rate: format.rateKhz, bits: format.bits, channels: format.channels })
    : format.kind === "dsd"
      ? t("signalPath.dsd", { level: format.level ?? "", rate: format.rateMhz, channels: format.channels })
      : t(format.kind === "unknown" ? "signalPath.unknownFormat" : "signalPath.noSignal");
  const mode = route ? t(`signalPath.${route.mode}`) : t("signalPath.unknownRoute");
  const source = information.source ? t(`player.source.${information.source}`) : t("signalPath.unknownSource");
  const state = t(`signalPath.${information.state}`);
  const secondary = information.state === "unavailable" ? state
    : information.state === "paused" || information.state === "stopped"
      ? [information.source ? source : null, state].filter(Boolean).join(" · ")
      : format.kind === "pcm" ? t("signalPath.pcmCompact", { rate: format.rateKhz, bits: format.bits })
        : format.kind === "dsd" ? t("signalPath.dsdCompact", { level: format.level ?? "", rate: format.rateMhz })
          : information.source ? source : "";
  const dsp = route?.mode === "shared" && status.shared_dsp?.in_path === true ? status.shared_dsp : null;
  const stages = [
    { label: "Sonante", icon: Music2 },
    { label: "MPD", icon: Cpu },
    ...(dsp ? [
      { label: "PCM 48 kHz", icon: AudioLines },
      { label: dsp.verified&&dsp.comparison?.side==="reference"?t("equalizer.compare.referenceInRoute"):t(`equalizer.state.${dsp.phase === "active" && (!dsp.verified || health.state !== "available" || status.state !== "play") ? "ready" : dsp.phase}`), icon: Cpu },
      { label: "PipeWire", icon: Volume2 },
    ] : [
      { label: route?.mode === "shared" ? "ALSA default" : route?.mode === "direct" && route.endpoint ? "ALSA hw" : "ALSA", icon: AudioLines },
      ...(route?.mode === "shared" ? [{ label: t("signalPath.system"), icon: Volume2 }] : []),
    ]),
    { label: route?.mode === "direct" ? "DAC" : t("signalPath.output"), icon: Speaker },
  ];
  if (compact) return <section aria-label={t("signalPath.title")} data-signal-summary
    className="flex min-w-0 flex-col items-start gap-0.5 text-[10px] leading-tight">
    <span className="max-w-full truncate text-[9px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.65))]" title={`${mode} · ${formatLabel} · ${state}`}>{mode}{secondary ? ` · ${secondary}` : ""}</span>
  </section>;
  return <section aria-label={t("signalPath.title")} data-signal-summary
    className={`relative w-full min-w-0 rounded-xl border border-[color:var(--theme-border-accent,#E5A00D)]/20 bg-[var(--theme-surface-subtle,rgb(0_0_0_/_0.25))] p-4 ${horizontal ? "np-signal-path" : ""}`}>
    <div className="np-signal-heading flex flex-wrap items-center justify-between gap-2">
      <h3 className="text-xs font-semibold text-[color:var(--theme-fg-primary,rgb(255_255_255_/_0.85))]">{t("signalPath.title")}</h3>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="text-[11px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.7))]">{mode} · {state}</span>
      </div>
    </div>
      {route && (
        <ol className="np-signal-stages mt-3 grid items-start gap-3" style={{ gridTemplateColumns: `repeat(${stages.length}, minmax(0, 1fr))` }} aria-label={t("signalPath.configured")}>
          {stages.map(({ label, icon: Icon }, index) => (
            <li key={index} className="relative flex min-w-0 flex-col items-center gap-2 text-center">
              {index > 0 && <ArrowRight size={12} className="absolute -left-2 top-1.5 text-[color:var(--theme-fg-accent,color-mix(in_srgb,#E5A00D_65%,transparent))]" aria-hidden="true" />}
              <Icon size={24} className="shrink-0 text-[color:var(--theme-fg-accent,#E5A00D)]" aria-hidden="true" />
              <span className="w-full break-words text-xs font-medium leading-snug text-[color:var(--theme-fg-primary,rgb(255_255_255_/_0.85))]">{label}</span>
            </li>
          ))}
        </ol>
      )}
      {!horizontal && route?.mode === "direct" && route.endpoint &&
        <p className="mt-2 break-all text-[11px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.7))]">{t("signalPath.endpointShort")}: {route.endpoint}</p>}
      <div className="np-signal-metadata mt-3 flex flex-wrap items-center gap-3 text-[11px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.75))]">
        <div className="np-signal-format rounded-lg border border-[color:var(--theme-border-accent,#E5A00D)]/20 bg-[#E5A00D]/5 px-3 py-2" title={t("signalPath.reported")}>
          <p className="np-signal-format-heading mb-1 flex flex-wrap items-center gap-x-1 text-[10px] font-semibold text-[color:var(--theme-fg-accent,#D1B475)]">
            {information.source && <><span>{source}</span><span aria-hidden="true">·</span></>}
            <span>{t("signalPath.reported")}</span>
          </p>
          {format.kind === "pcm" || format.kind === "dsd" ? <dl className="grid grid-cols-3 gap-x-3 gap-y-2">
            {(format.kind === "pcm"
              ? [["rate", `${format.rateKhz} kHz`], ["depth", `${format.bits}-bit`], ["channels", String(format.channels)]]
              : [["format", `DSD${format.level ?? ""}`], ["rate", `${format.rateMhz} MHz`], ["channels", String(format.channels)]]).map(([key, value]) => <div key={key}>
              <dt className="text-[10px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.6))]">{t(`signalPath.${key}`)}</dt><dd className="text-sm font-semibold tabular-nums text-[color:var(--theme-fg-primary,rgb(255_255_255_/_0.95))]">{value}</dd>
            </div>)}
          </dl> : <p className="text-xs text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.75))]">{formatLabel}</p>}
          {transport && <p data-signal-transport={transport} className="mt-1 text-[11px] text-[color:var(--theme-fg-accent,#D1B475)]">{t(`signalPath.transport.${transport}`)}</p>}
          {requiresDeviceVolumeControl(route, status) ? <p className="np-signal-format-volume mt-1 border-t border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] pt-1 text-[11px] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.75))]">{t("player.volumeOnDevice")}</p> : information.volumeBackend !== "unavailable" && <p className="np-signal-format-volume mt-1 border-t border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] pt-1 text-[11px] leading-snug text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.75))]">
            {t("signalPath.volume")}: {status.volume.available ? `${status.volume.value}% · ` : ""}{t(`player.volumeBackend.${information.volumeBackend}`)}
          </p>}
        </div>
        {observedOutput && <div data-alsa-observation="observed" className="min-w-0 rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.15))] px-3 py-2">
          <p className="mb-1 text-[10px] font-semibold text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.7))]">{t("signalPath.observation.observedTitle")}</p>
          <p className="break-words text-sm font-semibold tabular-nums text-[color:var(--theme-fg-primary,rgb(255_255_255_/_0.95))]">hw:{observedOutput.card},{observedOutput.device} · {observedOutput.format}</p>
          <p className="text-[11px]">{observedOutput.rate / 1000} kHz · {t("signalPath.observation.channelCount", { count: observedOutput.channels })}</p>
        </div>}
      </div>
      {route?.mode === "shared" && <section data-shared-dsp className="mt-3 rounded border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] p-3 text-xs text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.65))]">
        <p className="font-semibold">{t("equalizer.title")} · {t(`equalizer.state.${status.shared_dsp?.phase ?? "off"}`)}</p>
        {status.shared_dsp?.verified && status.shared_dsp.applied && <p>{status.shared_dsp.applied.name} · {t("equalizer.pcmFormat")} · {t("equalizer.stereo")}</p>}
        {status.shared_dsp?.verified && status.shared_dsp.protection && <p>{t("equalizer.appliedGain", { effective: status.shared_dsp.protection.effective_preamp_db.toFixed(1), requested: status.shared_dsp.protection.requested_preamp_db.toFixed(1) })}</p>}
        {status.shared_dsp?.comparison && <p>{t(status.shared_dsp.comparison.interrupted ? "equalizer.compare.interrupted" : status.shared_dsp.comparison.side === "reference" ? "equalizer.compare.referenceInRoute" : "equalizer.compare.equalized")}</p>}
        <p className="mt-1 text-[11px]">{t("equalizer.sharedProcessingHelp")}</p>
      </section>}
  </section>;
};
