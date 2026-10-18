import React from "react";
import { ArrowRight, AudioLines, BadgeCheck, CircleHelp, Cpu, Music2, Speaker, Volume2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";
import { routeConditions } from "../utils/routeConditions";
import { signalPathInformation, requiresDeviceVolumeControl } from "../utils/signalPath";

interface Props {
  route: ConfiguredSignalRoute | null;
  status: PlaybackStatus;
  health: MpdHealth;
  compact?: boolean;
  horizontal?: boolean;
}

// Conditions and evidence are shown only in the help dialog.
export const SignalPathDetails: React.FC<Props> = ({ route, status, health }) => {
  const { t } = useTranslation();
  const observation = status.output_observation;
  const observed = health.state === "available" && status.state === "play" && route?.mode === "direct"
    && observation?.state === "observed" && route.endpoint === observation.configured_endpoint ? observation : null;
  const observationReason = health.state !== "available" ? "unavailable"
    : status.state !== "play" ? "not_playing" : route?.mode === "shared" ? "shared"
      : observation?.state === "unverified" ? observation.reason : "unavailable";
  const outputText = observed
    ? `hw:${observed.card},${observed.device} · ${observed.format} · ${observed.rate / 1000} kHz · ${t("signalPath.observation.channelCount", { count: observed.channels })}`
    : t(`signalPath.observation.${observationReason}`);
  const observationTitle = t(observed ? "signalPath.observation.observedTitle" : "signalPath.observation.title");
  const conditions = routeConditions(route, status, health);
  const conditionsSummary = t(`signalPath.conditions.summary.${conditions.summary}`);
  return <section aria-label={t("signalPath.conditions.title")} className="min-w-0 space-y-4">
    {route?.mode === "shared" && <section data-shared-dsp className="rounded border border-white/10 p-3 text-xs text-white/65">
      <p className="font-semibold">{t("equalizer.title")} · {t(`equalizer.state.${status.shared_dsp?.phase ?? "off"}`)}</p>
      {status.shared_dsp?.verified && status.shared_dsp.applied && <p>{status.shared_dsp.applied.name} · {t("equalizer.pcmFormat")} · {t("equalizer.stereo")}</p>}
      {status.shared_dsp?.verified&&status.shared_dsp.protection&&<p>{t("equalizer.appliedGain",{effective:status.shared_dsp.protection.effective_preamp_db.toFixed(1),requested:status.shared_dsp.protection.requested_preamp_db.toFixed(1)})}</p>}
      {status.shared_dsp?.comparison&&<p>{t(status.shared_dsp.comparison.interrupted?"equalizer.compare.interrupted":status.shared_dsp.comparison.side==="reference"?"equalizer.compare.referenceInRoute":"equalizer.compare.equalized")}</p>}
      <p className="mt-1 text-[11px]">{t("equalizer.sharedProcessingHelp")}</p>
    </section>}
    <div data-alsa-observation={observed ? "observed" : "unverified"} className="mt-2 border-t border-white/10 pt-2 text-[11px] leading-relaxed text-white/75" title={t("signalPath.observation.help")}>
      <p className="font-semibold text-white/85">{observationTitle}</p>
      <p className="break-words">{outputText}</p>
      <p className="text-white/60">{t(conditions.bitPerfect ? "signalPath.seal.verified" : "signalPath.observation.notVerified")}</p>
      <div data-signal-quality-slot>
        {route?.mode === "direct" && conditions.assessment && <span
          data-route-assessment={conditions.assessment}
          title={t("signalPath.assessment.help")}
          className={`inline-flex rounded-full border px-2 py-1 text-[10px] leading-snug ${conditions.assessment === "compatible"
            ? "border-[#E5A00D]/35 bg-[#E5A00D]/10 text-[#D1B475]"
            : conditions.assessment === "attention" ? "border-amber-400/30 text-amber-200" : "border-white/15 text-white/55"}`}>
          {t(`signalPath.assessment.${conditions.assessment}`)}
        </span>}
        {route?.mode === "direct" && conditions.assessment !== "compatible" && conditions.issues?.[0] &&
          <p data-route-reason className="mt-1 text-[10px] leading-snug text-white/65">
            {t(`signalPath.conditions.label.${conditions.issues[0].label}`)}: {t(`signalPath.conditions.value.${conditions.issues[0].message}`, conditions.issues[0].values)}
          </p>}
      </div>

      <section data-route-conditions className="mt-4">
        <h3 className="font-semibold text-white/85">{t("signalPath.conditions.title")}</h3>
        <div tabIndex={0} role="region" aria-label={t("signalPath.conditions.title")} className="mt-2 rounded-lg border border-white/15 bg-[#17191d] p-3">
          <p className="mb-2 font-semibold text-white">{conditionsSummary}</p>
          {conditions.profile && <div data-route-profile={conditions.profile.id} className="mb-3 rounded border border-[#E5A00D]/25 bg-[#E5A00D]/5 p-2 text-white/80">
            <p className="font-semibold text-[#D1B475]">{t("signalPath.evidence.title")}</p>
            <p>{t("signalPath.evidence.reference", { frames: conditions.profile.measured.frames, tracks: conditions.profile.measured.tracks })}</p>
            <p className="mt-1 text-[10px] text-white/55">{conditions.profile.id} · {conditions.profile.measured.date}</p>
            <p className="mt-1 text-[10px] text-white/65">{t("signalPath.evidence.help")}</p>
          </div>}
          <dl className="space-y-2">
            {conditions.rows.map(row => <div key={row.label}>
              <dt className="text-white/55">{t(`signalPath.conditions.label.${row.label}`)}</dt>
              <dd className={row.tone === "attention" ? "text-amber-200" : "text-white/85"}>{t(`signalPath.conditions.value.${row.message}`, row.values)}</dd>
            </div>)}
          </dl>
          <p className="mt-3 border-t border-white/10 pt-2 text-white/60">{t("signalPath.conditions.help")}</p>
          <p className="mt-2 text-white/60">{t("signalPath.seal.scope")}</p>
        </div>
      </section>
    </div>
  </section>;
};

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
      className="shrink-0 rounded-full p-1.5 text-white/60 hover:bg-white/10 hover:text-white focus-visible:outline focus-visible:outline-[#E5A00D]">
      <CircleHelp size={18} aria-hidden="true" />
    </button>
    {open && <dialog ref={dialog} aria-label={title} aria-modal="true" tabIndex={-1}
      className="m-auto max-h-[85dvh] w-11/12 max-w-2xl overflow-y-auto rounded-xl border border-white/15 bg-[#181818] p-0 text-white/85 shadow-2xl backdrop:bg-black/75"
      onKeyDown={event => event.stopPropagation()}
      onCancel={event => { event.preventDefault(); setOpen(false); }} onClose={() => setOpen(false)}
      onClick={event => { event.stopPropagation(); if (event.target === event.currentTarget) setOpen(false); }}>
      <div className="p-5">
        <header className="mb-4 flex items-center justify-between gap-4">
          <h2 className="text-lg font-semibold">{title}</h2>
          <button type="button" autoFocus aria-label={closeLabel} onClick={() => setOpen(false)}
            className="rounded p-2 text-white/70 hover:bg-white/10 focus-visible:outline focus-visible:outline-[#E5A00D]"><X size={20} /></button>
        </header>
        {children}
      </div>
    </dialog>}
  </>;
}

export const SignalPath: React.FC<Props> = ({ route, status, health, compact = false, horizontal = false }) => {
  const { t } = useTranslation();
  const conditions = routeConditions(route, status, health);
  const information = signalPathInformation(status, health);
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
  const label = conditions.bitPerfect ? t("signalPath.seal.label")
    : conditions.assessment ? t(`signalPath.assessment.${conditions.assessment}`)
      : t(`signalPath.${information.state}`);
  const badge = <div data-signal-badge className="inline-flex max-w-full items-center gap-1">
    {route?.mode !== "shared" && <span data-route-assessment={conditions.assessment} data-bitperfect={conditions.bitPerfect ? "route" : undefined}
      className={`inline-flex min-w-0 items-center gap-1.5 rounded-full border px-2 py-1 ${conditions.bitPerfect
        ? "border-[#E5A00D]/40 bg-[#E5A00D]/10 text-[#E5B84C]" : conditions.assessment === "attention"
          ? "border-amber-400/30 text-amber-200" : "border-white/15 text-white/65"}`}>
      {conditions.bitPerfect && <BadgeCheck size={compact ? 13 : 16} aria-hidden="true" />}{label}
    </span>}
    <SignalPathInfo title={t("signalPath.modal.title")} openLabel={t("signalPath.modal.open")} closeLabel={t("signalPath.modal.close")}>
      <SignalPathDetails route={route} status={status} health={health} />
    </SignalPathInfo>
  </div>;
  if (compact) return <section aria-label={t("signalPath.title")} data-signal-summary
    className="flex min-w-0 flex-col items-start gap-0.5 text-[10px] leading-tight">
    {badge}
    <span className="max-w-full truncate text-[9px] text-white/65" title={`${mode} · ${formatLabel} · ${state}`}>{mode}{secondary ? ` · ${secondary}` : ""}</span>
  </section>;
  return <section aria-label={t("signalPath.title")} data-signal-summary
    className={`relative w-full min-w-0 rounded-xl border border-[#E5A00D]/20 bg-black/25 p-4 ${horizontal ? "np-signal-path" : ""}`}>
    <div className="np-signal-heading flex flex-wrap items-center justify-between gap-2">
      <h3 className="text-xs font-semibold text-white/85">{t("signalPath.title")}</h3>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="text-[11px] text-white/70">{mode} · {state}</span>
        {badge}
      </div>
    </div>
      {route && (
        <ol className={`np-signal-stages mt-3 grid items-start gap-3 ${route.mode === "shared" ? "grid-cols-5" : "grid-cols-4"}`} aria-label={t("signalPath.configured")}>
          {stages.map(({ label, icon: Icon }, index) => (
            <li key={index} className="relative flex min-w-0 flex-col items-center gap-2 text-center">
              {index > 0 && <ArrowRight size={12} className="absolute -left-2 top-1.5 text-[#E5A00D]/65" aria-hidden="true" />}
              <Icon size={24} className="shrink-0 text-[#E5A00D]" aria-hidden="true" />
              <span className="w-full break-words text-xs font-medium leading-snug text-white/85">{label}</span>
            </li>
          ))}
        </ol>
      )}
      {!horizontal && route?.mode === "direct" && route.endpoint &&
        <p className="mt-2 break-all text-[11px] text-white/70">{t("signalPath.endpointShort")}: {route.endpoint}</p>}
      <div className="np-signal-metadata mt-3 flex flex-wrap items-center gap-3 text-[11px] text-white/75">
        <div className="np-signal-format rounded-lg border border-[#E5A00D]/20 bg-[#E5A00D]/5 px-3 py-2" title={t("signalPath.reported")}>
          <p className="np-signal-format-heading mb-1 flex flex-wrap items-center gap-x-1 text-[10px] font-semibold text-[#D1B475]">
            {information.source && <><span>{source}</span><span aria-hidden="true">·</span></>}
            <span>{t("signalPath.reported")}</span>
          </p>
          {format.kind === "pcm" || format.kind === "dsd" ? <dl className="grid grid-cols-3 gap-x-3 gap-y-2">
            {(format.kind === "pcm"
              ? [["rate", `${format.rateKhz} kHz`], ["depth", `${format.bits}-bit`], ["channels", String(format.channels)]]
              : [["format", `DSD${format.level ?? ""}`], ["rate", `${format.rateMhz} MHz`], ["channels", String(format.channels)]]).map(([key, value]) => <div key={key}>
              <dt className="text-[10px] text-white/60">{t(`signalPath.${key}`)}</dt><dd className="text-sm font-semibold tabular-nums text-white/95">{value}</dd>
            </div>)}
          </dl> : <p className="text-xs text-white/75">{formatLabel}</p>}
          {conditions.transport && <p data-signal-transport={conditions.transport} className="mt-1 text-[11px] text-[#D1B475]">{t(`signalPath.transport.${conditions.transport}`)}</p>}
          {requiresDeviceVolumeControl(route, status) ? <p className="np-signal-format-volume mt-1 border-t border-white/10 pt-1 text-[11px] text-white/75">{t("player.volumeOnDevice")}</p> : information.volumeBackend !== "unavailable" && <p className="np-signal-format-volume mt-1 border-t border-white/10 pt-1 text-[11px] leading-snug text-white/75">
            {t("signalPath.volume")}: {status.volume.available ? `${status.volume.value}% · ` : ""}{t(`player.volumeBackend.${information.volumeBackend}`)}
          </p>}
        </div>
      </div>
  </section>;
};
