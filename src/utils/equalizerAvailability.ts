export type EqualizerActivationBlockReason =
  | "operation_pending"
  | "shared_route_required"
  | "availability_checking"
  | "engine_unavailable"
  | "catalog_loading"
  | "native_pulseaudio"
  | "runtime_unvalidated"
  | "runtime_unavailable";

export interface EqualizerActivationAvailability {
  shared: boolean;
  available: boolean | null;
  catalogLoading: boolean;
  runtimeAvailable: boolean | null;
  pulseAudio: boolean;
  pending: boolean;
}

/** Returns the first actionable reason EQ cannot be enabled. Unknown stays blocked. */
export function equalizerActivationBlockReason(
  state: EqualizerActivationAvailability,
): EqualizerActivationBlockReason | null {
  if (state.pending) return "operation_pending";
  if (!state.shared) return "shared_route_required";
  if (state.available === false) return "engine_unavailable";
  if (state.catalogLoading) return "catalog_loading";
  if (state.available === null) return "availability_checking";
  if (state.pulseAudio) return "native_pulseaudio";
  if (state.runtimeAvailable === null) return "runtime_unvalidated";
  if (!state.runtimeAvailable) return "runtime_unavailable";
  return null;
}
