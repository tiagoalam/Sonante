import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SlidersHorizontal } from "lucide-react";
import { equalizerService } from "../services/equalizer";
import type { AudioRouteProjection } from "../types/audio";
import type { DspSnapshot, EqCatalog, EqPreset } from "../types/equalizer";
import { equalizerIsActive, latestEqualizerSnapshot } from "../utils/audioExperience";
import { equalizerErrorCode, equalizerFailureMessageKey } from "../utils/equalizerError";
import { flatPreset, protection } from "../utils/equalizer";
import { equalizerActivationBlockReason } from "../utils/equalizerAvailability";

function activePreset(catalog: EqCatalog): EqPreset {
  const reference = catalog.config.active_preset;
  const preset = reference.kind === "user"
    ? catalog.config.user_presets.find(item => item.id === reference.id)
    : catalog.built_ins.find(item => item.id === reference.id);
  return preset ? { ...preset, bands: preset.bands.map(band => ({ ...band })) } : flatPreset();
}

export function EqualizerToggle({ shared, available = null, runtime, audioRoute, disabled = false, onAudioRouteConfirmed }: {
  shared: boolean;
  available?: boolean | null;
  runtime?: DspSnapshot | null;
  audioRoute?: AudioRouteProjection | null;
  disabled?: boolean;
  onAudioRouteConfirmed?: (projection: AudioRouteProjection, runtime: DspSnapshot) => void;
}) {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<EqCatalog | null>(null);
  const [localRuntime, setLocalRuntime] = useState<DspSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const [refreshingAvailability, setRefreshingAvailability] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [bypassConfirm, setBypassConfirm] = useState<EqPreset | null>(null);
  const alive = useRef(true);
  const busy = useRef(false);
  const operationGeneration = useRef(0);
  const catalogGeneration = useRef(0);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    alive.current = true;
    const request = ++catalogGeneration.current;
    equalizerService.get().then(value => {
      if (alive.current && catalogGeneration.current === request) setCatalog(value);
    }).catch(reason => {
      if (alive.current && catalogGeneration.current === request) setError(equalizerErrorCode(reason));
    });
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    if (!bypassConfirm || !dialog.current) return;
    dialog.current.showModal();
    return () => { if (dialog.current?.open) dialog.current.close(); };
  }, [bypassConfirm]);

  const externalRuntime = latestEqualizerSnapshot(runtime, catalog?.runtime);
  const localIsNewer = Boolean(localRuntime && (!externalRuntime || localRuntime.revision >= externalRuntime.revision));
  const current = latestEqualizerSnapshot(localRuntime, externalRuntime);
  const enabled = equalizerIsActive(current, localIsNewer ? null : audioRoute);
  const comparisonPreset = current?.applied ?? current?.comparison?.source ?? null;
  const applyingPreset = current?.phase === "updating" && current.verified && current.in_path === true && !!current.applied;
  const routeTransitioning = audioRoute?.operation === "transitioning" || audioRoute?.route?.state === "transitioning";
  const restoring = catalog?.config.enabled === true && !enabled && shared && current?.phase === "preparing" && !current.error;
  const restorePending = catalog?.config.enabled === true && !enabled && shared && current?.phase === "off" && !current.error;
  const restoreFailed = catalog?.config.enabled === true && !enabled && !!current?.error;
  const activationBlockReason = equalizerActivationBlockReason({
    shared,
    available,
    catalogLoading: !catalog || refreshingAvailability,
    runtimeAvailable: catalog?.runtime_available ?? null,
    pulseAudio: catalog?.pulse_audio ?? false,
    pending: pending || routeTransitioning || applyingPreset || restoring,
  });
  const cannotEnable = activationBlockReason !== null;
  const locked = disabled || pending || refreshingAvailability || routeTransitioning || applyingPreset || restoring || (!enabled && cannotEnable) || (!shared && enabled);

  const refreshCatalog = (generation: number) => {
    const request = ++catalogGeneration.current;
    void equalizerService.get().then(value => {
      if (!alive.current || operationGeneration.current !== generation || catalogGeneration.current !== request) return;
      setCatalog(value);
      setLocalRuntime(previous => !previous || value.runtime.revision >= previous.revision ? value.runtime : previous);
    }).catch(() => { /* The operation result remains authoritative if a catalog refresh is unavailable. */ });
  };

  const refreshAvailability = () => {
    if (busy.current || refreshingAvailability) return;
    const request = ++catalogGeneration.current;
    setRefreshingAvailability(true);
    setError(null);
    void equalizerService.get().then(value => {
      if (!alive.current || catalogGeneration.current !== request) return;
      setCatalog(value);
      setLocalRuntime(previous => !previous || value.runtime.revision >= previous.revision ? value.runtime : previous);
    }).catch(reason => {
      if (alive.current && catalogGeneration.current === request) setError(equalizerErrorCode(reason));
    }).finally(() => {
      if (alive.current && catalogGeneration.current === request) setRefreshingAvailability(false);
    });
  };

  const apply = async (nextEnabled: boolean, bypassExpected: EqPreset | null = null) => {
    if (busy.current || disabled || pending || refreshingAvailability) return;
    if (routeTransitioning) return;
    if (nextEnabled && cannotEnable) return;
    const generation = ++operationGeneration.current;
    busy.current = true;
    setPending(true);
    setError(null);
    try {
      const request = ++catalogGeneration.current;
      const fresh = await equalizerService.get();
      if (!alive.current) return;
      if (catalogGeneration.current !== request) return;
      setCatalog(fresh);
      const freshBlockReason = equalizerActivationBlockReason({
        shared, available, catalogLoading: false, runtimeAvailable: fresh.runtime_available,
        pulseAudio: fresh.pulse_audio, pending: false,
      });
      if (nextEnabled && freshBlockReason !== null) {
        const code = freshBlockReason === "native_pulseaudio"
          ? "EQ_PULSEAUDIO_UNSUPPORTED"
          : freshBlockReason === "shared_route_required"
            ? "EQ_SHARED_ONLY"
            : freshBlockReason === "engine_unavailable" || freshBlockReason === "runtime_unavailable"
              ? "EQ_RUNTIME_UNAVAILABLE"
              : "EQ_AVAILABILITY_UNCONFIRMED";
        throw new Error(code);
      }
      const result = await equalizerService.apply(nextEnabled, nextEnabled ? activePreset(fresh) : null, bypassExpected);
      if (alive.current && operationGeneration.current === generation) {
        setLocalRuntime(result.runtime);
        if (result.audioRoute) onAudioRouteConfirmed?.(result.audioRoute, result.runtime);
      }
      if (alive.current && operationGeneration.current === generation) refreshCatalog(generation);
    } catch (reason) {
      if (alive.current) {
        const code = equalizerErrorCode(reason);
        if (code === "EQ_PULSEAUDIO_UNSUPPORTED") {
          setCatalog(value => value ? { ...value, pulse_audio: true } : value);
          setError(null);
        } else {
          setError(code);
          if (operationGeneration.current === generation) refreshCatalog(generation);
        }
      }
    } finally {
      busy.current = false;
      if (alive.current) { setPending(false); setBypassConfirm(null); }
    }
  };

  const change = (nextEnabled: boolean) => {
    if (locked) return;
    if (!nextEnabled && enabled && comparisonPreset) {
      setBypassConfirm({ ...comparisonPreset, bands: comparisonPreset.bands.map(band => ({ ...band })) });
      return;
    }
    void apply(nextEnabled);
  };

  return (
    <section data-equalizer-toggle className="border-t border-[color:var(--theme-border-soft,#292929)] pt-4">
      <label className={`flex items-center gap-4 ${locked ? "cursor-not-allowed" : "cursor-pointer"}`}>
        <span aria-hidden="true" className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[var(--theme-surface-raised,#222222)] text-[color:var(--theme-fg-accent,#E5A00D)]"><SlidersHorizontal size={17} /></span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)]">{t("audioExperience.equalizerLabel")}</span>
          <span className="mt-0.5 block text-xs text-[color:var(--theme-fg-secondary,#AAAAAA)]">{t("audioExperience.equalizerDescription")}</span>
        </span>
        <input type="checkbox" role="switch" aria-label={t("audioExperience.equalizerLabel")} checked={enabled} disabled={locked}
          onChange={event => change(event.currentTarget.checked)}
          className="h-5 w-9 shrink-0 cursor-pointer appearance-none rounded-full bg-[var(--theme-border-soft,#444444)] outline-none transition-colors checked:bg-[var(--theme-control-accent,#E5A00D)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] before:block before:h-4 before:w-4 before:translate-x-0.5 before:translate-y-0.5 before:rounded-full before:bg-white before:transition-transform checked:before:translate-x-[18px] disabled:cursor-not-allowed disabled:opacity-45" />
      </label>
      {!enabled && activationBlockReason === "native_pulseaudio" && <div role="status" data-eq-block-reason="native_pulseaudio" className="ml-[52px] mt-2 space-y-1 text-xs text-[color:var(--theme-fg-secondary,#AAAAAA)]">
        <strong className="block text-[color:var(--theme-fg-primary,#ffffff)]">{t("equalizer.pulseAudioTitle")}</strong>
        <p>{t("equalizer.pulseAudioNotice")}</p>
        <button type="button" disabled={pending || refreshingAvailability} onClick={refreshAvailability} className="underline underline-offset-2 disabled:opacity-50">{t("equalizer.recheckAvailability")}</button>
      </div>}
      {!enabled && activationBlockReason && activationBlockReason !== "operation_pending" && activationBlockReason !== "native_pulseaudio" && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t(`equalizer.activationBlocked.${activationBlockReason}`)}</p>}
      {pending && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t(enabled ? "audioExperience.status.disablingEq" : "audioExperience.status.enablingEq")}</p>}
      {!pending && applyingPreset && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.applyingPreset")}</p>}
      {!pending && restoring && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.restoringEq")}</p>}
      {!pending && restorePending && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.eqRequested")}</p>}
      {!pending && restoreFailed && <p role="alert" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-error,#FF8686)]">{t("audioExperience.status.eqRestoreFailed")}</p>}
      {error && <div role="alert" className="ml-[52px] mt-2 flex flex-wrap items-center gap-x-2 text-xs text-[color:var(--theme-fg-error,#FF8686)]">
        <span>{t(equalizerFailureMessageKey(error))}</span>
        <details><summary className="cursor-pointer underline underline-offset-2">{t("audioExperience.status.details")}</summary><code className="mt-1 block">{t("equalizer.diagnosticCode", { code: error })}</code></details>
      </div>}
      {bypassConfirm && <dialog ref={dialog} aria-modal="true" aria-labelledby="settings-eq-bypass-title"
        onCancel={event => { event.preventDefault(); if (!pending) setBypassConfirm(null); }}
        className="m-auto max-w-md rounded-xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#1b1c1f)] p-5 text-[color:var(--theme-fg-primary,#ffffff)] shadow-xl backdrop:bg-black/65">
        <h2 id="settings-eq-bypass-title" className="font-semibold">{t("equalizer.bypassConfirmTitle")}</h2>
        <p className="mt-3 text-sm leading-relaxed text-[color:var(--theme-fg-secondary,#AAAAAA)]">{t("equalizer.bypassConfirmHelp", { db: Math.abs(protection(bypassConfirm)?.effective_preamp_db ?? 0).toFixed(1) })}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" disabled={pending} onClick={() => setBypassConfirm(null)} className="rounded-lg border border-[color:var(--theme-border-soft,#444444)] px-3 py-2 text-xs">{t("equalizer.cancel")}</button>
          <button type="button" disabled={pending} onClick={() => void apply(false, bypassConfirm)} className="rounded-lg bg-[#E5A00D] px-3 py-2 text-xs text-black disabled:opacity-40">{t("equalizer.confirmBypass")}</button>
        </div>
      </dialog>}
    </section>
  );
}
