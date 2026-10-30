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
    source: available && (status.current_media?.kind === "local" || status.current_media?.kind === "plex")
      ? status.current_media.kind : null,
    format: available && state !== "stopped" ? parseReportedMpdFormat(status.audio_format) : { kind: "absent" } as ReportedMpdFormat,
    volumeBackend: available ? status.volume.backend : "unavailable",
  };
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
