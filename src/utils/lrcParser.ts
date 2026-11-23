/** Pure parser for the conventional, line-synchronized LRC subset. */

export const LRC_LIMITS = Object.freeze({
  maxBytes: 512 * 1024,
  maxLines: 10_000,
  maxLineCodeUnits: 8_192,
  maxTimestampsPerLine: 32,
  maxTimestampMs: 7 * 24 * 60 * 60 * 1_000,
  maxOffsetMs: 10 * 60 * 1_000,
});

export type LrcErrorCode =
  | "invalid_input"
  | "input_too_large"
  | "too_many_lines"
  | "line_too_large"
  | "too_many_timestamps"
  | "invalid_timestamp"
  | "timestamp_out_of_range"
  | "invalid_offset"
  | "offset_out_of_range"
  | "unsupported_word_sync"
  | "duplicate_metadata"
  | "empty_content";

export class LrcParseError extends Error {
  readonly code: LrcErrorCode;
  readonly line?: number;

  constructor(code: LrcErrorCode, line?: number) {
    super(code);
    this.name = "LrcParseError";
    this.code = code;
    this.line = line;
  }
}

export interface LrcMetadata {
  readonly artist?: string;
  readonly title?: string;
  readonly album?: string;
  /** Preserved only; its playback-clock convention is intentionally undefined. */
  readonly offsetMs?: number;
  /** Other syntactically valid metadata tags, in source order. */
  readonly other: readonly { readonly tag: string; readonly value: string; readonly sourceLine: number }[];
}

export interface LrcSourceLine {
  /** Exact line content, excluding the original newline delimiter. */
  readonly text: string;
  readonly index: number;
}

export interface LrcEvent {
  readonly timestampMs: number;
  readonly text: string;
  readonly sourceLine: number;
  readonly timestampIndex: number;
  /** Stable order for equal timestamps. */
  readonly order: number;
}

export type LrcParseResult =
  | { readonly kind: "synchronized"; readonly sourceText: string; readonly sourceLines: readonly LrcSourceLine[]; readonly metadata: LrcMetadata; readonly events: readonly LrcEvent[] }
  | { readonly kind: "plain"; readonly sourceText: string; readonly sourceLines: readonly LrcSourceLine[]; readonly metadata: LrcMetadata; readonly text: string };

const WORD_SYNC = /<\d+:\d{2}(?:\.\d{1,3})?>/;
const METADATA_LINE = /^\[([a-zA-Z][a-zA-Z0-9_-]*):(.*?)\]$/;
const TIMESTAMP_PREFIX = /^\[(\d+):(\d{2})(?:\.(\d+))?\]/;
const TIMESTAMP_LIKE_PREFIX = /^\[\d+:/;

function fail(code: LrcErrorCode, line?: number): never {
  throw new LrcParseError(code, line);
}

function parseTimestamp(minuteText: string, secondText: string, fraction: string | undefined, line: number): number {
  if (fraction !== undefined && (fraction.length < 1 || fraction.length > 3)) fail("invalid_timestamp", line);
  const minutes = Number(minuteText);
  const seconds = Number(secondText);
  if (!Number.isSafeInteger(minutes) || seconds > 59) fail("invalid_timestamp", line);
  const millis = fraction === undefined ? 0 : Number(fraction.padEnd(3, "0"));
  const result = (minutes * 60 + seconds) * 1_000 + millis;
  if (!Number.isSafeInteger(result)) fail("invalid_timestamp", line);
  if (result > LRC_LIMITS.maxTimestampMs) fail("timestamp_out_of_range", line);
  return result;
}

/**
 * Parse conventional LRC into source-preserving lines and derived events.
 * Enhanced LRC word timing is rejected instead of being mistaken for plain text.
 * Throws only LrcParseError for invalid or over-limit input.
 */
export function parseLrc(input: string): LrcParseResult {
  if (typeof input !== "string") fail("invalid_input");
  if (new TextEncoder().encode(input).byteLength > LRC_LIMITS.maxBytes) fail("input_too_large");

  // Split CRLF, LF, and CR while retaining each line's exact text.
  const rawLines = input.split(/\r\n|\n|\r/);
  if (rawLines.length > LRC_LIMITS.maxLines) fail("too_many_lines");
  const sourceLines: LrcSourceLine[] = rawLines.map((text, index) => {
    if (text.length > LRC_LIMITS.maxLineCodeUnits) fail("line_too_large", index + 1);
    return { text, index };
  });

  const metadata: { artist?: string; title?: string; album?: string; offsetMs?: number; other: { tag: string; value: string; sourceLine: number }[] } = { other: [] };
  const seenMetadata = new Set<string>();
  const events: LrcEvent[] = [];
  let order = 0;
  let usefulPlainText = false;

  for (const source of sourceLines) {
    const lineNumber = source.index + 1;
    const text = source.text;
    if (WORD_SYNC.test(text)) fail("unsupported_word_sync", lineNumber);

    const metadataMatch = METADATA_LINE.exec(text);
    if (metadataMatch) {
      const tag = metadataMatch[1].toLowerCase();
      const value = metadataMatch[2];
      if (["ar", "ti", "al", "offset"].includes(tag)) {
        if (seenMetadata.has(tag)) fail("duplicate_metadata", lineNumber);
        seenMetadata.add(tag);
      }
      if (tag === "ar") metadata.artist = value;
      else if (tag === "ti") metadata.title = value;
      else if (tag === "al") metadata.album = value;
      else if (tag === "offset") {
        if (!/^[+-]?\d+$/.test(value)) fail("invalid_offset", lineNumber);
        const offset = Number(value);
        if (!Number.isSafeInteger(offset)) fail("invalid_offset", lineNumber);
        if (Math.abs(offset) > LRC_LIMITS.maxOffsetMs) fail("offset_out_of_range", lineNumber);
        metadata.offsetMs = offset;
      } else metadata.other.push({ tag, value, sourceLine: source.index });
      continue;
    }

    let rest = text;
    const timestamps: number[] = [];
    while (rest.startsWith("[")) {
      const match = TIMESTAMP_PREFIX.exec(rest);
      if (!match) {
        if (TIMESTAMP_LIKE_PREFIX.test(rest)) fail("invalid_timestamp", lineNumber);
        break;
      }
      timestamps.push(parseTimestamp(match[1], match[2], match[3], lineNumber));
      if (timestamps.length > LRC_LIMITS.maxTimestampsPerLine) fail("too_many_timestamps", lineNumber);
      rest = rest.slice(match[0].length);
    }

    if (timestamps.length) {
      for (let timestampIndex = 0; timestampIndex < timestamps.length; timestampIndex++) {
        events.push({ timestampMs: timestamps[timestampIndex], text: rest, sourceLine: source.index, timestampIndex, order: order++ });
      }
    } else if (text.trim().length > 0) {
      usefulPlainText = true;
    }
  }

  if (!events.length && !usefulPlainText) fail("empty_content");

  const metadataResult: LrcMetadata = {
    ...(metadata.artist === undefined ? {} : { artist: metadata.artist }),
    ...(metadata.title === undefined ? {} : { title: metadata.title }),
    ...(metadata.album === undefined ? {} : { album: metadata.album }),
    ...(metadata.offsetMs === undefined ? {} : { offsetMs: metadata.offsetMs }),
    other: metadata.other.map(tag => ({ ...tag })),
  };
  const preservedLines = sourceLines.map(line => ({ ...line }));

  if (!events.length) {
    return { kind: "plain", sourceText: input, sourceLines: preservedLines, metadata: metadataResult,
      text: sourceLines.filter(line => line.text.trim().length > 0 && !METADATA_LINE.test(line.text)).map(line => line.text).join("\n") };
  }
  events.sort((a, b) => a.timestampMs - b.timestampMs || a.order - b.order);
  return { kind: "synchronized", sourceText: input, sourceLines: preservedLines, metadata: metadataResult,
    events: events.map(event => ({ ...event })) };
}
