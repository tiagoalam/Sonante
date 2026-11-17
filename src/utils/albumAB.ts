export type ABSide = 'a' | 'b';
export type ABSeconds = 30 | 60 | 90;
export interface ABTrack { key: string; title: string; duration: number | null; number: number }
export interface ABSample { left: Float32Array<ArrayBuffer>; right: Float32Array<ArrayBuffer>; frames: number; sourceRate: number; sourceChannels: number; start: number; convertedFromDsd: boolean }
export const AB_RATE = 48_000;
export function abSeconds(value: number): value is ABSeconds { return value === 30 || value === 60 || value === 90; }
export function abStart(value: string): number | null {
  const text = value.trim(); if (!/^\d{1,3}:[0-5]\d(?:\.\d{1,3})?$/u.test(text)) return null;
  const [minutes, seconds] = text.split(':').map(Number); const total = minutes * 60 + seconds;
  return total <= 36_000 ? total : null;
}
export function decodeABSample(raw: ArrayBuffer): ABSample {
  if (!(raw instanceof ArrayBuffer) || raw.byteLength < 40) throw 'AB_DECODE';
  const view = new DataView(raw), magic = String.fromCharCode(...new Uint8Array(raw, 0, 4));
  const rate = view.getUint32(4, true), channels = view.getUint32(8, true), frames = view.getUint32(12, true);
  const sourceRate = view.getUint32(16, true), sourceChannels = view.getUint32(20, true), start = view.getUint32(24, true) / 1000;
  const flags = view.getUint32(36, true), convertedFromDsd = (flags & 1) !== 0;
  if (magic !== 'SAB1' || rate !== AB_RATE || channels !== 2 || frames < AB_RATE * 3 || frames > AB_RATE * 93
    || raw.byteLength !== 40 + frames * 8 || sourceRate < 8000
    || (convertedFromDsd ? sourceRate !== 2_822_400 : sourceRate > 192000)
    || (flags & ~1) !== 0 || ![1, 2].includes(sourceChannels) || start > 36_000) throw 'AB_DECODE';
  const left = new Float32Array(frames), right = new Float32Array(frames);
  for (let i = 0; i < frames; i++) {
    const a = view.getFloat32(40 + i * 8, true), b = view.getFloat32(44 + i * 8, true);
    if (!Number.isFinite(a) || !Number.isFinite(b) || Math.abs(a) > 8 || Math.abs(b) > 8) throw 'AB_DECODE';
    left[i] = a; right[i] = b;
  }
  return { left, right, frames, sourceRate, sourceChannels, start, convertedFromDsd };
}
/** Crop a common, already-prepared timeline. Positive offsets advance B; negative offsets advance A. */
export function alignABSamples(a: ABSample, b: ABSample, targetFrames: number, offsetFrames: number): [ABSample, ABSample] | null {
  if (!Number.isSafeInteger(targetFrames) || targetFrames < AB_RATE * 3 || targetFrames > AB_RATE * 90
    || !Number.isSafeInteger(offsetFrames) || Math.abs(offsetFrames) > AB_RATE * 3) return null;
  const shiftA = Math.max(0, -offsetFrames), shiftB = Math.max(0, offsetFrames);
  if (a.frames < shiftA + targetFrames || b.frames < shiftB + targetFrames
    || a.left.length < a.frames || a.right.length < a.frames || b.left.length < b.frames || b.right.length < b.frames) return null;
  const crop = (sample: ABSample, shift: number): ABSample => ({
    ...sample,
    left: sample.left.subarray(shift, shift + targetFrames),
    right: sample.right.subarray(shift, shift + targetFrames),
    frames: targetFrames,
    start: sample.start + shift / AB_RATE,
  });
  return [crop(a, shiftA), crop(b, shiftB)];
}
export function cropABSamples(a: ABSample, b: ABSample, targetFrames: number): [ABSample, ABSample] | null {
  if (!Number.isSafeInteger(targetFrames) || targetFrames < AB_RATE * 3 || targetFrames > AB_RATE * 90
    || a.frames < targetFrames || b.frames < targetFrames
    || a.left.length < a.frames || a.right.length < a.frames || b.left.length < b.frames || b.right.length < b.frames) return null;
  return [
    { ...a, left: a.left.subarray(0, targetFrames), right: a.right.subarray(0, targetFrames), frames: targetFrames },
    { ...b, left: b.left.subarray(0, targetFrames), right: b.right.subarray(0, targetFrames), frames: targetFrames },
  ];
}
export function abLevels(a: ABSample, b: ABSample, match: boolean): [number, number] {
  const frames = Math.min(a.frames, b.frames), stats = (s: ABSample) => {
    let sum = 0, peak = 0;
    for (let i = 0; i < frames; i++) { const l = s.left[i], r = s.right[i]; sum += l * l + r * r; peak = Math.max(peak, Math.abs(l), Math.abs(r)); }
    return { rms: Math.sqrt(sum / (frames * 2)), peak };
  };
  const x = stats(a), y = stats(b), target = Math.min(x.rms, y.rms);
  const gains: [number, number] = match && target > 0.00001 ? [target / x.rms, target / y.rms] : [1, 1];
  const peak = Math.max(x.peak * gains[0], y.peak * gains[1]), ceiling = peak > 0.95 ? 0.95 / peak : 1;
  return [gains[0] * ceiling, gains[1] * ceiling];
}
export interface ABFailure { code: string; codec?: string; sampleRate?: number; channels?: number; side?: ABSide }
const failureCode = (value: unknown): string => typeof value === 'string' && /^AB_(?:BUSY|SHARED_ONLY|SOURCE|CHANGED|SAME_TRACK|TRACKS|NETWORK|LIMIT|STORAGE|FORMAT|DECODE|SEEK|TOO_SHORT|INVALID|PAUSE|UNAVAILABLE|CONTAINER|CODEC|SAMPLE_RATE|CHANNELS|FORMAT_INFO|FORMAT_CHANGE|RESAMPLE|DSD_RATE|DSD_FORMAT|DSD_CLIPPING)$/u.test(value) ? value.slice(3).toLowerCase() : 'unavailable';
export function abFailure(error: unknown, side?: ABSide): ABFailure {
  const value = error && typeof error === 'object' ? error as Record<string, unknown> : null;
  const code = failureCode(value ? value.code : error);
  const result: ABFailure = { code, ...(side ? { side } : {}) };
  if (!value || code === 'unavailable') return result;
  if (typeof value.codec === 'string' && ['PCM', 'FLAC', 'ALAC', 'MP1', 'MP2', 'MP3', 'AAC', 'Vorbis', 'Opus', 'WavPack', 'APE', 'TTA', 'WMA', 'DTS', 'E-AC-3', 'AC-4', 'Speex', 'Musepack', 'DSD (DSF)', 'DSD (DSDIFF)'].includes(value.codec)) result.codec = value.codec;
  if (typeof value.sample_rate === 'number' && Number.isSafeInteger(value.sample_rate) && value.sample_rate > 0 && value.sample_rate <= 20_000_000) result.sampleRate = value.sample_rate;
  if (typeof value.channels === 'number' && Number.isSafeInteger(value.channels) && value.channels > 0 && value.channels <= 64) result.channels = value.channels;
  return result;
}
export const abError = (error: unknown): string => abFailure(error).code;

export function decodeABTracks(raw: unknown): ABTrack[] {
 if (!Array.isArray(raw) || raw.length === 0 || raw.length > 500) throw 'AB_TRACKS';
 const seen = new Set<string>();
 return raw.map(row => {
  if (!row || typeof row !== 'object' || typeof row.key !== 'string' || !/^[a-f0-9]{64}$/u.test(row.key) || seen.has(row.key)
   || typeof row.title !== 'string' || !row.title.trim() || row.title.length > 4096 || /[\u0000-\u001f\u007f]/u.test(row.title)
   || !Number.isSafeInteger(row.number) || row.number < 1 || row.number > 500
   || (row.duration !== null && (typeof row.duration !== 'number' || !Number.isFinite(row.duration) || row.duration <= 0))) throw 'AB_TRACKS';
  seen.add(row.key); return { key: row.key, title: row.title, number: row.number, duration: row.duration };
 });
}
