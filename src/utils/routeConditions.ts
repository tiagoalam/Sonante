import type { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";
import { parseReportedMpdFormat } from "./signalPath.ts";
import { engineProfiles, identifiedRouteEngine, reportsDsdTransport, strictDirectEngine, identifiedUsb, usbMatches } from "./routeEvidenceProfiles.ts";
import type { RouteEvidenceProfile } from "./routeEvidenceProfiles.ts";

export interface RouteCondition {
  label: "negotiation" | "transport" | "engine" | "device" | "kernel" | "route" | "rate" | "channels" | "container" | "volume" | "replayGain" | "crossfade" | "mixramp";
  message: string;
  tone: "info" | "attention" | "unknown";
  values?: Record<string, string | number>;
}
const capacity: Record<string, number> = { S8:8, U8:8, S16_LE:16, S16_BE:16, S24_LE:24, S24_BE:24, S24_3LE:24, S24_3BE:24, S32_LE:32, S32_BE:32 };
const validNumber = (n: unknown, max: number): n is number => typeof n === "number" && Number.isFinite(n) && n >= 0 && n <= max;

// Read-only route conditions. A bit-perfect route indication additionally needs
// the owned, identified engine whose relevant output path was tested. It is not a
// comparison of current samples or a claim about the DAC internal processing.
export type MonitoredAssessment = "compatible" | "attention" | "partial";
interface RouteConditionsResult {
  summary: "inactive" | "attention" | "partial" | "preserved";
  rows: RouteCondition[];
  assessment?: MonitoredAssessment;
  bitPerfect?: boolean;
  transport?: "native_dsd" | "dop";
  profile?: RouteEvidenceProfile;
  issues?: RouteCondition[];
}

function physicalEndpoint(endpoint: string | null | undefined): boolean {
  if (typeof endpoint !== "string") return false;
  const match = /^hw:(?:CARD=)?([A-Za-z0-9_-]{1,32}),(?:DEV=)?([0-9]{1,3})$/.exec(endpoint);
  return !!match && match[0] === endpoint && Number(match[2]) <= 255;
}

export function routeConditions(route: ConfiguredSignalRoute | null, status: PlaybackStatus, health: MpdHealth): RouteConditionsResult {
  const rows: RouteCondition[] = [];
  const add = (label: RouteCondition["label"], message: string, tone: RouteCondition["tone"] = "unknown", values?: RouteCondition["values"]) => rows.push({ label, message, tone, values });
  if (health.state !== "available" || status.state !== "play") return { summary: "inactive", rows };
  if (status.snapshot_consistent === false) {
    add("route", "snapshotChanged");
    return { summary: "partial", assessment: "partial", bitPerfect: false, rows };
  }
  const evidence = status.output_observation;
  const observed = route?.mode === "direct" && physicalEndpoint(route.endpoint)
    && evidence?.state === "observed" && evidence.configured_endpoint === route.endpoint
    && Number.isInteger(evidence.card) && evidence.card >= 0 && evidence.card <= 1023
    && Number.isInteger(evidence.device) && evidence.device >= 0 && evidence.device <= 255
    && Number.isInteger(evidence.subdevice) && evidence.subdevice >= 0 && evidence.subdevice <= 31
    && Number.isInteger(evidence.rate) && evidence.rate >= 1000 && evidence.rate <= 50000000
    && Number.isInteger(evidence.channels) && evidence.channels >= 1 && evidence.channels <= 64
    && typeof evidence.format === "string" ? evidence : null;
  add("route", route?.mode === "shared" ? "routeShared" : observed ? "routeObserved" : "notChecked", observed ? "info" : "unknown");
  const input = parseReportedMpdFormat(status.audio_format);
  const pcm = input.kind === "pcm" && observed && !observed.format.startsWith("DSD_");
  const negotiated = status.processing?.transport;
  const nativeBits = observed ? ({DSD_U8:8, DSD_U16_LE:16, DSD_U16_BE:16, DSD_U32_LE:32, DSD_U32_BE:32} as Record<string, number>)[observed.format] : undefined;
  const sourceDsdRate = input.kind === "dsd" ? Math.round(input.rateMhz * 1000000) : 0;
  const negotiatedDsd = input.kind === "dsd" && !!observed && reportsDsdTransport(status.engine_evidence)
    && negotiated && Number.isSafeInteger(negotiated.source_rate) && (negotiated.kind === "native_dsd" || negotiated.kind === "dop")
    && Number.isSafeInteger(negotiated.epoch) && negotiated.epoch > 0
    && negotiated.source_rate * 8 === sourceDsdRate
    && negotiated.output_rate === observed.rate && negotiated.channels === observed.channels;
  const nativeDsd = !!negotiatedDsd && negotiated?.kind === "native_dsd" && !!nativeBits;
  const dop = !!negotiatedDsd && negotiated?.kind === "dop" && !!observed && [24,32].includes(capacity[observed.format]);
  const dsd = nativeDsd || dop;
  if (input.kind === "dsd") add("transport", dsd ? nativeDsd ? "nativeDsd" : "dop" : "dsdTransportUnknown", dsd ? "info" : "unknown");
  if (pcm && input.kind === "pcm" && observed) {
    const sourceRate = input.rateKhz * 1000;
    add("rate", sourceRate === observed.rate ? "rateEqual" : "rateDifferent", sourceRate === observed.rate ? "info" : "attention", { input: input.rateKhz, output: observed.rate / 1000 });
    add("channels", input.channels === observed.channels ? "channelsEqual" : "channelsDifferent", input.channels === observed.channels ? "info" : "attention", { input: input.channels, output: observed.channels });
    const bits = capacity[observed.format];
    add("container", !bits ? "containerUnknown" : bits < input.bits ? "containerSmaller" : bits > input.bits ? "containerWider" : "containerEqual",
      bits && bits < input.bits ? "attention" : "unknown", bits ? { input: input.bits, output: bits } : undefined);
  } else if (dsd && input.kind === "dsd" && observed) {
    const outputDsdRate = observed.rate * (nativeDsd ? nativeBits! : 16);
    add("rate", outputDsdRate === sourceDsdRate ? "dsdRateEqual" : "dsdRateDifferent", outputDsdRate === sourceDsdRate ? "info" : "attention", { input: input.rateMhz, output: outputDsdRate / 1000000, carrier: observed.rate / 1000 });
    add("channels", input.channels === observed.channels ? "channelsEqual" : "channelsDifferent", input.channels === observed.channels ? "info" : "attention", { input: input.channels, output: observed.channels });
    add("container", nativeDsd ? "nativeDsdContainer" : "dopContainer", "info", { format: observed.format });
  } else {
    const reason = input.kind === "dsd" || observed?.format.startsWith("DSD_") ? "dsdSeparate" : "notChecked";
    for (const label of ["rate", "channels", "container"] as const) add(label, reason);
  }
  // These identifiers are context, not a DAC/kernel whitelist for playback.
  if (route?.mode === "direct") add("negotiation", strictDirectEngine(status.engine_evidence) ? "directStrict" : "directCompatibility", strictDirectEngine(status.engine_evidence) ? "info" : "unknown");
  const candidates = engineProfiles(status.engine_evidence);
  add("engine", observed && identifiedRouteEngine(status.engine_evidence) ? "engineIdentified" : "notChecked", observed && identifiedRouteEngine(status.engine_evidence) ? "info" : "unknown");
  const usb = observed?.usb_device;
  add("device", identifiedUsb(usb) ? "deviceObserved" : "notChecked", identifiedUsb(usb) ? "info" : "unknown",
    identifiedUsb(usb) ? { model: usb.model, vendor: usb.vendor_id, product: usb.product_id } : undefined);
  const kernel = observed?.kernel_release;
  const validKernel = typeof kernel === "string" && kernel.length > 0 && kernel.length <= 128 && !/[^A-Za-z0-9._+-]/.test(kernel);
  add("kernel", validKernel ? "kernelObserved" : "notChecked", validKernel ? "info" : "unknown", validKernel ? { value: kernel } : undefined);
  // Historical sample-exact tests are additional evidence only. A different
  // DAC, kernel or supported PCM format must not fail the live assessment.
  const profile = candidates.find(p => identifiedUsb(usb) && usbMatches(p, usb)
    && validKernel && p.kernels.includes(kernel) && pcm && input.kind === "pcm"
    && input.bits === p.input.bits && input.rateKhz * 1000 === p.input.rate && input.channels === p.input.channels
    && observed && observed.rate === p.output.rate && observed.channels === p.output.channels && observed.format === p.output.format);
  const processing = status.processing;
  const volume = processing?.mpd_volume;
  // The badge describes the route configuration during an intentional mute,
  // not sample identity of the silence being played. Require coherent readings.
  const muted = status.volume.backend === "mpd_software" && status.volume.available
    && status.volume.muted === true && status.volume.value === 0 && volume === 0;
  if (status.volume.backend === "mpd_software" && status.volume.available && validNumber(volume,100) && Number.isInteger(volume)) {
    if ((dsd || volume === 100) && status.volume.value !== volume) add("volume", "volumeChanged");
    else if (muted) add("volume", "softwareMuted", "info");
    else if (dsd) add("volume", volume > 0 ? "dsdSoftwareBypass" : "dsdMuted", volume > 0 ? "info" : "attention");
    else add("volume", (pcm || dsd) ? volume < 100 ? "softwareVolume" : "softwareUnity" : "softwareOther", (pcm || dsd) && volume < 100 ? "attention" : "unknown", { value: volume });
  } else add("volume", status.volume.backend === "alsa_hardware" ? "hardwareVolume" : status.volume.backend === "pipewire" ? "systemVolume" : "notChecked");
  const gain = processing?.replay_gain;
  if (!gain || gain.state === "unknown") add("replayGain", "notChecked");
  else if (gain.state === "unavailable") add("replayGain", "replayGainUnavailable");
  else add("replayGain", `gain_${gain.state}`, gain.state === "off" ? "info" : "attention");
  const crossfade = processing?.crossfade_seconds;
  if (validNumber(crossfade,86400)) add("crossfade", crossfade === 0 ? "crossfadeOff" : "crossfadeOn", crossfade === 0 ? "info" : "attention", { value: crossfade });
  else add("crossfade", "notChecked");
  add("mixramp", processing?.mixramp === "off" ? "mixrampOff" : processing?.mixramp === "possible" ? "mixrampPossible" : "notChecked", processing?.mixramp === "off" ? "info" : "unknown");
  const attention = rows.some(row => row.tone === "attention");
  // Evaluate only current, observable format/processing conditions. This reports
  // necessary conditions; it never claims that the current samples were tested.
  const expected = [
    ["route", ["routeObserved"]], ["rate", ["rateEqual", "dsdRateEqual"]], ["channels", ["channelsEqual"]],
    ["container", ["containerEqual", "containerWider", "nativeDsdContainer", "dopContainer"]], ["volume", ["softwareUnity", "dsdSoftwareBypass", "softwareMuted"]],
    ["replayGain", ["gain_off"]], ["crossfade", ["crossfadeOff"]], ["mixramp", ["mixrampOff"]],
  ] as const;
  const issues = rows.filter(row => row.tone === "attention");
  for (const [label, messages] of expected) {
    const row = rows.find(row => row.label === label);
    if (row && !messages.some(message => message === row.message) && !issues.includes(row)) issues.push(row);
  }
  const compatible = !attention && !!observed && (!!pcm || dsd) && issues.length === 0
    && status.volume.backend === "mpd_software" && status.volume.available === true && (muted || status.volume.value === 100 || (dsd && status.volume.value > 0));
  return {
    summary: attention ? "attention" : compatible ? "preserved" : "partial", rows,
    assessment: attention ? "attention" : compatible ? "compatible" : "partial",
    bitPerfect: compatible && identifiedRouteEngine(status.engine_evidence) && (dsd || (input.kind === "pcm" && [8, 16, 24, 32].includes(input.bits))),
    transport: dsd ? nativeDsd ? "native_dsd" : "dop" : undefined,
    profile: compatible ? profile : undefined, issues,
  };
}
