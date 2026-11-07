import type { AppConfig } from "../types/config";
import type { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";

// Only a known ALSA hardware identifier may cross the window boundary here.
// Arbitrary PCM names can contain private paths or plugin configuration.
export function configuredSignalRoute(config: AppConfig | null): ConfiguredSignalRoute | null {
  if (!config || config.first_run) return null;
  const shared = config.audio_output_type === "pipewire" || config.alsa_device === "default";
  const endpoint = /^hw:(?:CARD=)?[A-Za-z0-9_]+,(?:DEV=)?\d+$/.test(config.alsa_device)
    ? config.alsa_device : null;
  return { mode: shared ? "shared" : "direct", endpoint: shared ? "default" : endpoint };
}

export type ReportedMpdFormat =
  | { kind: "pcm"; rateKhz: number; bits: number; channels: number }
  | { kind: "dsd"; rateMhz: number; level: number | null; channels: number }
  | { kind: "unknown" }
  | { kind: "absent" };

// MPD's decoded status format is not evidence of the original file codec,
// native DSD, or the signal delivered to a DAC. Unknown values stay unknown.
export function parseReportedMpdFormat(value: string): ReportedMpdFormat {
  if (!value) return { kind: "absent" };
  const dsd = /^dsd(32|64|128|256|512|1024):(\d+)$/.exec(value);
  const dsdBytes = /^(\d+):dsd:(\d+)$/.exec(value);
  if (dsd || dsdBytes) {
    const channels = Number((dsd ?? dsdBytes)![2]);
    // MPD's long DSD format counts bytes/s; the short form names a 44.1 kHz multiple.
    // This is decoded MPD metadata, independent of the output mode and source badges.
    const rate = dsd ? 44_100 * Number(dsd[1]) : Number(dsdBytes![1]) * 8;
    if (!Number.isSafeInteger(rate) || rate < 1_000_000 || rate > 50_000_000
      || !Number.isSafeInteger(channels) || channels < 1 || channels > 64) return { kind: "unknown" };
    const level = [32, 64, 128, 256, 512, 1024].find(level => 44_100 * level === rate) ?? null;
    return { kind: "dsd", rateMhz: rate / 1_000_000, level, channels };
  }
  const match = /^(\d+):(8|16|24|32):(\d+)$/.exec(value);
  if (!match) return { kind: "unknown" };
  const rate = Number(match[1]), channels = Number(match[3]);
  if (rate < 1000 || rate > 1536000 || channels < 1 || channels > 64) return { kind: "unknown" };
  return { kind: "pcm", rateKhz: rate / 1000, bits: Number(match[2]), channels };
}

export function signalPathInformation(status: PlaybackStatus, health: MpdHealth) {
  const available = health.state === "available";
  const state = !available ? "unavailable" : status.state === "play" ? "playing"
    : status.state === "pause" ? "paused" : "stopped";
  return {
    state,
    source: available && (status.current_media?.kind === "local" || status.current_media?.kind === "plex" || status.current_media?.kind === "upnp" || status.current_media?.kind === "radio")
      ? status.current_media.kind : null,
    format: available && state !== "stopped" ? parseReportedMpdFormat(status.audio_format) : { kind: "absent" } as ReportedMpdFormat,
    volumeBackend: available ? status.volume.backend : "unavailable",
  };
}

export function observedAlsaOutput(route: ConfiguredSignalRoute | null, status: PlaybackStatus, health: MpdHealth) {
  const output = status.output_observation;
  const endpoint = route?.endpoint;
  const match = typeof endpoint === "string" ? /^hw:(?:CARD=)?[A-Za-z0-9_-]{1,32},(?:DEV=)?([0-9]{1,3})$/.exec(endpoint) : null;
  if (health.state !== "available" || status.state !== "play" || status.snapshot_consistent === false
    || route?.mode !== "direct" || !match || Number(match[1]) > 255
    || output?.state !== "observed" || output.configured_endpoint !== endpoint
    || !Number.isInteger(output.card) || output.card < 0 || output.card > 1023
    || !Number.isInteger(output.device) || output.device < 0 || output.device > 255
    || !Number.isInteger(output.subdevice) || output.subdevice < 0 || output.subdevice > 31
    || !Number.isInteger(output.rate) || output.rate < 1000 || output.rate > 50000000
    || !Number.isInteger(output.channels) || output.channels < 1 || output.channels > 64
    || typeof output.format !== "string" || output.format.length === 0) return null;
  return output;
}

export function observedDsdTransport(status: PlaybackStatus, output: NonNullable<ReturnType<typeof observedAlsaOutput>>) {
  const input = parseReportedMpdFormat(status.audio_format);
  const transport = status.processing?.transport;
  const engine = status.engine_evidence;
  const identified = engine?.state === "identified" && engine.pcm_eof_flush === true && (
    (engine.evidence_id === "mpd-route-20261017" && engine.scope === "pcm_dsd_route")
    || (["mpd-strict-route-20261017", "mpd-strict-ubuntu2204-remap-20261018"].includes(engine.evidence_id)
      && engine.scope === "pcm_dsd_route_strict"));
  if (input.kind !== "dsd" || !identified || !transport
    || (transport.kind !== "native_dsd" && transport.kind !== "dop")
    || !Number.isSafeInteger(transport.source_rate) || transport.source_rate * 8 !== Math.round(input.rateMhz * 1_000_000)
    || !Number.isSafeInteger(transport.epoch) || transport.epoch <= 0
    || transport.output_rate !== output.rate || transport.channels !== output.channels) return undefined;
  const nativeFormat = ["DSD_U8", "DSD_U16_LE", "DSD_U16_BE", "DSD_U32_LE", "DSD_U32_BE"].includes(output.format);
  const dopFormat = ["S24_LE", "S24_BE", "S24_3LE", "S24_3BE", "S32_LE", "S32_BE"].includes(output.format);
  if (transport.kind === "native_dsd" && nativeFormat) return "native_dsd" as const;
  if (transport.kind === "dop" && dopFormat) return "dop" as const;
  return undefined;
}

// Direct uses equipment volume for every transport, including PCM.
export function requiresDeviceVolumeControl(route: ConfiguredSignalRoute | null | undefined, status: PlaybackStatus | null | undefined): boolean {
  return route?.mode === "direct" || status?.volume.backend === "direct_equipment";
}

export function canToggleMute(status: PlaybackStatus | null | undefined): boolean {
  const volume = status?.volume;
  return !!volume?.available && (volume.backend === "direct_equipment" ? !!volume.direct_token : volume.writable);
}

export function muteLabel(status: PlaybackStatus | null | undefined): string {
 if(status?.volume.backend==="direct_equipment")return status.volume.muted?"player.unmute":"player.mute";
 return status?.volume.muted||status?.volume.value===0?"player.unmute":"player.mute";
}
export function directMuteRequest(status:PlaybackStatus){
 if(status.volume.backend!=="direct_equipment"||!canToggleMute(status))return null;
 return{muted:!status.volume.muted,token:status.volume.direct_token!};
}

export function sharedMuteRequest(status:PlaybackStatus){
 const v=status.volume;
 if(!v.available||!v.writable||!v.shared_token||!['pipewire','mpd_software'].includes(v.backend))return null;
 return{muted:!(v.muted||v.value===0),token:v.shared_token};
}
