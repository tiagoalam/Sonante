import type { EngineEvidence, OutputObservation } from "../types/audio";

export interface RouteEvidenceProfile {
  readonly id: string;
  readonly engine: { readonly evidenceId: string; readonly scope: string };
  readonly usb: { readonly vendor: string; readonly product: string; readonly model: string };
  readonly kernels: readonly string[];
  readonly input: { readonly bits: number; readonly rate: number; readonly channels: number };
  readonly output: { readonly format: string; readonly rate: number; readonly channels: number };
  readonly measured: { readonly frames: number; readonly tracks: number; readonly date: string };
}

// Only completed, sample-exact USB reference tests belong here. Adding a DAC
// does not change generic observation or imply support for unmeasured formats.
export const ROUTE_EVIDENCE_PROFILES: readonly RouteEvidenceProfile[] = [{
  id: "usb-e70-pcm24-96-20261017",
  engine: { evidenceId: "mpd-pcm-eof-20261017", scope: "pcm24_96000_stereo_gapless" },
  usb: { vendor: "152a", product: "8750", model: "E70" },
  kernels: ["6.12.108-1-MANJARO"],
  input: { bits: 24, rate: 96000, channels: 2 },
  output: { format: "S32_LE", rate: 96000, channels: 2 },
  measured: { frames: 192000, tracks: 2, date: "2026-10-17" },
}];

type Usb = Extract<OutputObservation, { state: "observed" }>["usb_device"];

export function identifiedUsb(usb: Usb): usb is NonNullable<Usb> {
  return !!usb && typeof usb.vendor_id === "string" && usb.vendor_id.length === 4 && !/[^0-9a-f]/.test(usb.vendor_id)
    && typeof usb.product_id === "string" && usb.product_id.length === 4 && !/[^0-9a-f]/.test(usb.product_id)
    && typeof usb.model === "string" && usb.model.length > 0 && usb.model.length <= 128
    && !/[\x00-\x1f\x7f]/.test(usb.model);
}

export function usbMatches(profile: RouteEvidenceProfile, usb: Usb): boolean {
  return identifiedUsb(usb) && usb.vendor_id === profile.usb.vendor
    && usb.product_id === profile.usb.product && usb.model === profile.usb.model;
}

export function engineProfiles(engine: EngineEvidence | null | undefined): readonly RouteEvidenceProfile[] {
  return engine?.state === "identified" && engine.pcm_eof_flush === true
    ? ROUTE_EVIDENCE_PROFILES.filter(profile => profile.engine.evidenceId === engine.evidence_id
      && profile.engine.scope === engine.scope) : [];
}

// Build identity is independent of historical per-device USB reference tests.
export function identifiedRouteEngine(engine: EngineEvidence | null | undefined): boolean {
  return engine?.state === "identified" && engine.pcm_eof_flush === true && (
    (engine.evidence_id === "mpd-pcm-eof-20261017" && engine.scope === "pcm24_96000_stereo_gapless") ||
    (engine.evidence_id === "mpd-route-20261017" && engine.scope === "pcm_dsd_route")
  );
}
export function reportsDsdTransport(engine: EngineEvidence | null | undefined): boolean {
  return identifiedRouteEngine(engine) && engine?.evidence_id === "mpd-route-20261017";
}
