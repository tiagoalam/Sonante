import type { AudioRoute, AudioRouteSnapshot } from "./audioRoute";
import type { AudioRouteProjection } from "../types/audio";
import type { DspSnapshot } from "../types/equalizer";
import type { AudioExperienceMode } from "../components/AudioModeSelector";

export type AudioRouteStatusKind = "requested" | "transitioning" | "confirmed" | "failed" | "failed_recovered" | "uncertain";

export function audioModeForRoute(route: AudioRoute | null | undefined): AudioExperienceMode {
  if (!route) return "shared";
  if (route.processing === "equalizer") return "equalized";
  return route.access === "exclusive" && route.destination.kind === "device" ? "direct" : "shared";
}

export function audioRouteStatusKind(projection: AudioRouteProjection | null | undefined): AudioRouteStatusKind {
  if (!projection) return "uncertain";
  const route: AudioRouteSnapshot | null = projection.route;
  if (projection.operation === "transitioning" || route?.state === "transitioning") return "transitioning";
  if (projection.operation === "failed") return route?.confirmed ? "failed_recovered" : "failed";
  if (projection.operation === "uncertain" || !route || route.state === "uncertain") return "uncertain";
  if (route.state === "confirmed" && route.confirmed) return "confirmed";
  if (route.state === "requested") return "requested";
  return "uncertain";
}

export function audioRouteDiagnosticKey(projection: AudioRouteProjection | null | undefined): string | null {
  const diagnostic = projection?.diagnostic;
  return diagnostic ? `audioExperience.diagnostic.${diagnostic}` : null;
}

/** Preferences alone never imply that the DSP route is active. */
export function equalizerIsActive(
  runtime: DspSnapshot | null | undefined,
  projection?: AudioRouteProjection | null,
): boolean {
  const runtimeActive = runtime?.verified === true && runtime.in_path === true
    && (runtime.phase === "active" || runtime.phase === "ready" && Boolean(runtime.applied || runtime.comparison)
      || runtime.phase === "updating" && Boolean(runtime.applied || runtime.comparison));
  const confirmedRouteActive = projection?.route?.confirmed?.access === "shared"
    && projection.route.confirmed.processing === "equalizer"
    && projection.route.state !== "uncertain";
  return runtimeActive || confirmedRouteActive;
}

/** Newer DSP observations win; late polling responses must not revive stale Off. */
export function latestEqualizerSnapshot(
  ...snapshots: Array<DspSnapshot | null | undefined>
): DspSnapshot | null {
  return snapshots.filter((snapshot): snapshot is DspSnapshot => Boolean(snapshot))
    .reduce<DspSnapshot | null>((latest, snapshot) => !latest || snapshot.revision >= latest.revision ? snapshot : latest, null);
}
