import { AlertCircle, CheckCircle2, CircleHelp, LoaderCircle } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AudioRouteProjection, OutputObservation } from "../types/audio";
import { audioModeForRoute, audioRouteStatusKind } from "../utils/audioExperience";
import type { AudioExperienceMode } from "./AudioModeSelector";

export function AudioRouteStatus({ projection, selectedMode, pendingSave = false, playing = false, outputObservation = null }: {
  projection?: AudioRouteProjection | null;
  selectedMode: AudioExperienceMode;
  pendingSave?: boolean;
  playing?: boolean;
  outputObservation?: OutputObservation | null;
}) {
  const { t } = useTranslation();
  const routeState = projection ? audioRouteStatusKind(projection) : null;
  const requested = projection?.route?.requested;
  const directAwaitingEvidence = routeState === "requested"
    && projection?.diagnostic === "direct_pcm_not_observed"
    && requested?.access === "exclusive" && requested.destination.kind === "device";
  const directEvidenceState = directAwaitingEvidence && playing
    ? outputObservation?.state === "observed" ? "confirming" : "uncertain"
    : null;
  const state = directEvidenceState ?? routeState;
  const confirmed = projection?.route?.confirmed ?? null;
  const title = !projection ? pendingSave ? t("audioExperience.status.pendingSave") : null
    : state === "transitioning"
    ? t("audioExperience.status.changing")
    : state === "confirming"
      ? t("audioExperience.status.confirming")
    : state === "failed" || state === "failed_recovered"
      ? t("audioExperience.status.failed")
      : state === "uncertain"
        ? t("audioExperience.status.uncertain")
        : pendingSave ? t("audioExperience.status.pendingSave")
          : directAwaitingEvidence ? t("audioExperience.status.directAwaitingPlayback")
          : state === "requested" ? t("audioExperience.status.requested")
          : confirmed
          ? t(`audioExperience.status.${audioModeForRoute(confirmed) === "direct" ? "direct" : "shared"}`)
          : null;
  if (!title) return null;

  const failed = state === "failed" || state === "failed_recovered";
  const Icon = state === "transitioning" || state === "confirming" ? LoaderCircle
    : failed || state === "uncertain" ? AlertCircle
      : directAwaitingEvidence && !playing ? CircleHelp : CheckCircle2;
  const diagnostic = projection?.diagnostic
    ? t(`audioExperience.diagnostic.${projection.diagnostic}`, { defaultValue: t("audioExperience.diagnostic.generic") })
    : null;

  return (
    <div data-audio-route-status data-status={state ?? "pending_save"} className="flex min-w-0 items-center gap-2 text-xs">
      <Icon size={14} aria-hidden="true" className={`shrink-0 ${state === "transitioning" || state === "confirming" ? "animate-spin" : failed || state === "uncertain" ? "text-[color:var(--theme-fg-error,#FF8686)]" : "text-[color:var(--theme-fg-muted,#999999)]"}`} />
      <span role={failed ? "alert" : "status"} aria-live="polite" data-audio-route-state={state}
        className={failed ? "text-[color:var(--theme-fg-error,#FF8686)]" : "text-[color:var(--theme-fg-secondary,#AAAAAA)]"}>{title}</span>
      {(diagnostic || state === "failed_recovered") && (
        <details className="min-w-0 text-[color:var(--theme-fg-muted,#888888)]">
          <summary className="cursor-pointer underline underline-offset-2">{t("audioExperience.status.details")}</summary>
          <div className="mt-1 max-w-lg space-y-1 leading-relaxed">
            {state === "failed_recovered" && <p>{t("audioExperience.status.recovered")}</p>}
            {diagnostic && <p>{diagnostic}</p>}
          </div>
        </details>
      )}
      {pendingSave && <span className="sr-only" data-selected-audio-mode={selectedMode}>{t(`audioExperience.mode.${selectedMode}`)}</span>}
    </div>
  );
}
