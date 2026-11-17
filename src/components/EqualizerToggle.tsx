import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SlidersHorizontal } from "lucide-react";
import { equalizerService } from "../services/equalizer";
import type { AudioRouteProjection } from "../types/audio";
import type { DspSnapshot, EqCatalog, EqPreset } from "../types/equalizer";
import { equalizerIsActive, latestEqualizerSnapshot } from "../utils/audioExperience";
import { equalizerErrorCode } from "../utils/equalizerError";
import { flatPreset, protection } from "../utils/equalizer";

function activePreset(catalog: EqCatalog): EqPreset {
  const reference = catalog.config.active_preset;
  const preset = reference.kind === "user"
    ? catalog.config.user_presets.find(item => item.id === reference.id)
    : catalog.built_ins.find(item => item.id === reference.id);
  return preset ? { ...preset, bands: preset.bands.map(band => ({ ...band })) } : flatPreset();
}

export function EqualizerToggle({ shared, available = true, runtime, audioRoute, disabled = false }: {
  shared: boolean;
  available?: boolean;
  runtime?: DspSnapshot | null;
  audioRoute?: AudioRouteProjection | null;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<EqCatalog | null>(null);
  const [localRuntime, setLocalRuntime] = useState<DspSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [bypassConfirm, setBypassConfirm] = useState<EqPreset | null>(null);
  const alive = useRef(true);
  const busy = useRef(false);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    alive.current = true;
    equalizerService.get().then(value => { if (alive.current) setCatalog(value); })
      .catch(reason => { if (alive.current) setError(equalizerErrorCode(reason)); });
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
  const restoring = catalog?.config.enabled === true && !enabled && shared && current?.phase === "preparing" && !current.error;
  const restorePending = catalog?.config.enabled === true && !enabled && shared && current?.phase === "off" && !current.error;
  const restoreFailed = catalog?.config.enabled === true && !enabled && !!current?.error;
  const cannotEnable = !shared || !available || !catalog?.runtime_available || catalog.pulse_audio;
  const locked = disabled || pending || applyingPreset || restoring || !catalog || (!enabled && cannotEnable) || (!shared && enabled);

  const apply = async (nextEnabled: boolean, bypassExpected: EqPreset | null = null) => {
    if (busy.current || disabled || pending) return;
    if (nextEnabled && cannotEnable) return;
    busy.current = true;
    setPending(true);
    setError(null);
    try {
      const fresh = await equalizerService.get();
      if (!alive.current) return;
      setCatalog(fresh);
      if (nextEnabled && (!shared || !available || !fresh.runtime_available || fresh.pulse_audio)) {
        throw new Error(fresh.pulse_audio ? "EQ_PULSEAUDIO_UNSUPPORTED" : "EQ_SHARED_ONLY");
      }
      const result = await equalizerService.apply(nextEnabled, nextEnabled ? activePreset(fresh) : null, bypassExpected);
      if (alive.current) setLocalRuntime(result);
      try {
        const confirmed = await equalizerService.get();
        if (alive.current) setCatalog(confirmed);
      } catch { /* Runtime confirmation succeeded; keep the last catalog if a refresh fails. */ }
    } catch (reason) {
      if (alive.current) {
        const code = equalizerErrorCode(reason);
        if (code === "EQ_PULSEAUDIO_UNSUPPORTED") setCatalog(value => value ? { ...value, pulse_audio: true } : value);
        setError(code);
        try {
          const recovered = await equalizerService.get();
          if (alive.current) { setCatalog(recovered); setLocalRuntime(recovered.runtime); }
        } catch { /* Keep the last runtime snapshot when recovery observation is unavailable. */ }
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
      {!shared && <p className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.equalizerSharedOnly")}</p>}
      {pending && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t(enabled ? "audioExperience.status.disablingEq" : "audioExperience.status.enablingEq")}</p>}
      {!pending && applyingPreset && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.applyingPreset")}</p>}
      {!pending && restoring && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.restoringEq")}</p>}
      {!pending && restorePending && <p role="status" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("audioExperience.status.eqRequested")}</p>}
      {!pending && restoreFailed && <p role="alert" className="ml-[52px] mt-1 text-xs text-[color:var(--theme-fg-error,#FF8686)]">{t("audioExperience.status.eqRestoreFailed")}</p>}
      {error && <div role="alert" className="ml-[52px] mt-2 flex flex-wrap items-center gap-x-2 text-xs text-[color:var(--theme-fg-error,#FF8686)]">
        <span>{t("audioExperience.status.equalizerFailed")}</span>
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
