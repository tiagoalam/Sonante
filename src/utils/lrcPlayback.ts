import type { LrcParseResult } from "./lrcParser";

/**
 * Return the event currently represented by the playback snapshot. The parser's
 * offset is intentionally metadata-only until its clock convention is defined.
 */
export function activeLrcEventIndex(result: LrcParseResult, positionMs: number): number | null {
  if (result.kind !== "synchronized" || !Number.isSafeInteger(positionMs) || positionMs < 0) return null;
  let active: number | null = null;
  for (let index = 0; index < result.events.length; index++) {
    if (result.events[index].timestampMs > positionMs) break;
    active = index;
  }
  return active;
}
