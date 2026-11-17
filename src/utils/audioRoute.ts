export type AudioDestination =
  | { kind: "system_default" }
  | { kind: "device"; deviceId: string };

export type AudioAccess = "shared" | "exclusive";
export type AudioProcessing = "off" | "equalizer";
export type AudioRouteState = "requested" | "transitioning" | "confirmed" | "uncertain";

export interface AudioRoute {
  destination: AudioDestination;
  access: AudioAccess;
  processing: AudioProcessing;
}

export type LegacyRouteIssue =
  | "legacy_alsa_default_maps_to_shared"
  | "explicit_device_ignored_in_shared_mode"
  | "unknown_output_type_uses_legacy_mapping"
  | "invalid_device_identifier";

export interface LegacyRouteInterpretation {
  route: AudioRoute;
  issues: LegacyRouteIssue[];
}

export interface AudioRouteSnapshot {
  state: AudioRouteState;
  requested: AudioRoute;
  confirmed: AudioRoute | null;
}

/** Reject an out-of-order projection response without interpreting its route. */
export function acceptsAudioRouteGeneration(candidate: number, latest: number): boolean {
  return Number.isSafeInteger(candidate) && candidate >= latest;
}

export type RouteContractError =
  | "invalid_device_identifier"
  | "exclusive_system_default"
  | "equalizer_requires_shared_access"
  | "invalid_snapshot_state"
  | "requested_and_observed_route_differ";

export function validateAudioRoute(route: AudioRoute): RouteContractError | null {
  if (route.destination.kind === "device"
    && (!route.destination.deviceId || route.destination.deviceId === "default"
      || /[\u0000-\u001f\u007f]/u.test(route.destination.deviceId))) {
    return "invalid_device_identifier";
  }
  if (route.destination.kind === "system_default" && route.access === "exclusive") {
    return "exclusive_system_default";
  }
  if (route.processing === "equalizer" && route.access !== "shared") {
    return "equalizer_requires_shared_access";
  }
  return null;
}

export function sameAudioRoute(a: AudioRoute, b: AudioRoute): boolean {
  const sameDestination = a.destination.kind === b.destination.kind
    && (a.destination.kind === "system_default"
      || (b.destination.kind === "device" && a.destination.deviceId === b.destination.deviceId));
  return sameDestination && a.access === b.access && a.processing === b.processing;
}

export function interpretLegacyAudioRoute(
  outputType: string | null | undefined,
  device: string | null | undefined,
): LegacyRouteInterpretation {
  const type = outputType ?? "alsa";
  const endpoint = device ?? "default";
  const knownType = ["alsa", "pipewire", "shared"].includes(type);
  const sharedMode = type === "pipewire" || type === "shared";
  const usesSharedAccess = sharedMode || endpoint === "default";
  const issues: LegacyRouteIssue[] = [];

  if (!knownType) issues.push("unknown_output_type_uses_legacy_mapping");
  if (type === "alsa" && endpoint === "default") issues.push("legacy_alsa_default_maps_to_shared");
  if (sharedMode && endpoint !== "default") issues.push("explicit_device_ignored_in_shared_mode");
  if (!usesSharedAccess && (!endpoint || /[\u0000-\u001f\u007f]/u.test(endpoint))) {
    issues.push("invalid_device_identifier");
  }

  return {
    route: {
      destination: usesSharedAccess ? { kind: "system_default" } : { kind: "device", deviceId: endpoint },
      access: usesSharedAccess ? "shared" : "exclusive",
      // Legacy output settings do not establish that DSP is running.
      processing: "off",
    },
    issues,
  };
}

export function requestedRouteSnapshot(route: AudioRoute): AudioRouteSnapshot {
  const error = validateAudioRoute(route);
  if (error) throw new Error(error);
  return { state: "requested", requested: route, confirmed: null };
}

export function transitioningRouteSnapshot(
  requested: AudioRoute,
  previouslyConfirmed: AudioRoute | null = null,
): AudioRouteSnapshot {
  const error = validateAudioRoute(requested) ?? (previouslyConfirmed && validateAudioRoute(previouslyConfirmed));
  if (error) throw new Error(error);
  return { state: "transitioning", requested, confirmed: previouslyConfirmed };
}

export function confirmedRouteSnapshot(requested: AudioRoute, observed: AudioRoute): AudioRouteSnapshot {
  const error = validateAudioRoute(requested) ?? validateAudioRoute(observed);
  if (error) throw new Error(error);
  if (!sameAudioRoute(requested, observed)) throw new Error("requested_and_observed_route_differ");
  return { state: "confirmed", requested, confirmed: observed };
}

export function uncertainRouteSnapshot(requested: AudioRoute): AudioRouteSnapshot {
  const error = validateAudioRoute(requested);
  if (error) throw new Error(error);
  return { state: "uncertain", requested, confirmed: null };
}

export function validateAudioRouteSnapshot(snapshot: AudioRouteSnapshot): RouteContractError | null {
  const requestedError = validateAudioRoute(snapshot.requested);
  if (requestedError) return requestedError;
  if (snapshot.confirmed) {
    const confirmedError = validateAudioRoute(snapshot.confirmed);
    if (confirmedError) return confirmedError;
  }
  if ((snapshot.state === "requested" || snapshot.state === "uncertain") && snapshot.confirmed !== null) {
    return "invalid_snapshot_state";
  }
  if (snapshot.state === "confirmed" && (!snapshot.confirmed || !sameAudioRoute(snapshot.requested, snapshot.confirmed))) {
    return "invalid_snapshot_state";
  }
  return null;
}
