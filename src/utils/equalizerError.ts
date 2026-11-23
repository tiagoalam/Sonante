/** Return only a stable backend error identifier; never expose arbitrary error text or paths. */
export function equalizerErrorCode(reason: unknown): string {
  let value = typeof reason === "string" ? reason : "";
  if (!value && reason instanceof Error && isEqMessageCode(reason.message)) value = reason.message;
  if (!value && reason && typeof reason === "object") {
    const record = reason as Record<string, unknown>;
    if (typeof record.code === "string") value = record.code;
    else if (typeof record.message === "string" && isEqMessageCode(record.message)) value = record.message;
  }
  const candidate = value.split(/[;\s]/, 1)[0];
  return /^EQ_[A-Za-z][A-Za-z0-9_]{0,63}$/.test(candidate) || candidate === "continuity_lost"
    ? candidate
    : "EQ_UNCONFIRMED";
}

export function equalizerFailureMessageKey(code: string): string {
  if (code === "EQ_PULSEAUDIO_UNSUPPORTED") return "equalizer.pulseAudioNotice";
  if (code === "EQ_SinkUnavailable") return "audioExperience.status.outputUnavailable";
  if (code === "EQ_PipeWireUnavailable") return "audioExperience.status.audioServerUnavailable";
  return "audioExperience.status.equalizerFailed";
}

function isEqMessageCode(value: string): boolean {
  const candidate = value.split(/[;\s]/, 1)[0];
  return /^EQ_[A-Za-z][A-Za-z0-9_]{0,63}$/.test(candidate);
}
