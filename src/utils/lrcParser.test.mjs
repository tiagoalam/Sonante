import test from "node:test";
import assert from "node:assert/strict";
import { LRC_LIMITS, LrcParseError, parseLrc } from "./lrcParser.ts";

const code = (code, input) => assert.throws(() => parseLrc(input), error => {
  assert.ok(error instanceof LrcParseError);
  assert.equal(error.code, code);
  return true;
});

test("conventional synchronized LRC parses centiseconds and milliseconds into integer events", () => {
  const parsed = parseLrc("[00:01.2]One\n[01:23.45]Two\n[02:03.456]Three");
  assert.equal(parsed.kind, "synchronized");
  assert.deepEqual(parsed.events.map(event => event.timestampMs), [1_200, 83_450, 123_456]);
  assert.ok(parsed.events.every(event => Number.isInteger(event.timestampMs)));
});

test("multiple timestamps duplicate a lyric line and stable sorting preserves source order for ties", () => {
  const parsed = parseLrc("[00:02.00][00:01.00]First\n[00:01.00]Second");
  assert.equal(parsed.kind, "synchronized");
  assert.deepEqual(parsed.events.map(({ timestampMs, text, sourceLine, timestampIndex }) => ({ timestampMs, text, sourceLine, timestampIndex })), [
    { timestampMs: 1_000, text: "First", sourceLine: 0, timestampIndex: 1 },
    { timestampMs: 1_000, text: "Second", sourceLine: 1, timestampIndex: 0 },
    { timestampMs: 2_000, text: "First", sourceLine: 0, timestampIndex: 0 },
  ]);
  assert.deepEqual(parsed.events.map(event => event.order), [1, 2, 0]);
});

test("metadata and original Unicode source lines are preserved without applying offset", () => {
  const source = "[ar:Zoë & 藤井]\r\n[ti:Canção — 東京]\r\n[al:Álbum]\r\n[offset:-125]\r\n[by:fixture]\r\n[00:03.25]♫ Olá, 世界!";
  const copy = source;
  const parsed = parseLrc(source);
  assert.equal(parsed.kind, "synchronized");
  assert.deepEqual(parsed.metadata, { artist: "Zoë & 藤井", title: "Canção — 東京", album: "Álbum", offsetMs: -125,
    other: [{ tag: "by", value: "fixture", sourceLine: 4 }] });
  assert.equal(parsed.events[0].timestampMs, 3_250);
  assert.equal(parsed.events[0].text, "♫ Olá, 世界!");
  assert.equal(parsed.sourceText, copy);
  assert.deepEqual(parsed.sourceLines.map(line => line.text), source.split("\r\n"));
  assert.equal(source, copy);
});

test("plain text stays unsynchronized and blank lines do not gain timestamps", () => {
  const parsed = parseLrc("[ti:Only metadata]\n\n  Plain line\n第二行");
  assert.equal(parsed.kind, "plain");
  assert.equal(parsed.text, "  Plain line\n第二行");
  assert.deepEqual(parsed.sourceLines.map(line => line.text), ["[ti:Only metadata]", "", "  Plain line", "第二行"]);
});

test("conventional positive and negative offsets are preserved as metadata", () => {
  for (const [raw, expected] of [["+250", 250], ["-250", -250], ["0", 0]]) {
    const parsed = parseLrc(`[offset:${raw}]\n[00:00.00]line`);
    assert.equal(parsed.kind, "synchronized");
    assert.equal(parsed.metadata.offsetMs, expected);
    assert.equal(parsed.events[0].timestampMs, 0);
  }
});

test("malformed timestamps and Enhanced LRC word timing are rejected with typed errors", () => {
  code("invalid_timestamp", "[00:60.00]bad");
  code("invalid_timestamp", "[00:01.1234]bad");
  code("invalid_timestamp", "[00:xx.00]bad");
  code("unsupported_word_sync", "[00:01.00]word <00:01.25> timing");
});

test("invalid, duplicate, and out-of-range offset metadata is rejected", () => {
  code("invalid_offset", "[offset:1.5]\nline");
  code("offset_out_of_range", `[offset:${LRC_LIMITS.maxOffsetMs + 1}]\nline`);
  code("duplicate_metadata", "[ar:first]\n[ar:second]\nline");
});

test("timestamp, input, line, and per-line timestamp limits are enforced atomically", () => {
  code("timestamp_out_of_range", "[10081:00]late");
  code("input_too_large", "x".repeat(LRC_LIMITS.maxBytes + 1));
  code("line_too_large", `x${"y".repeat(LRC_LIMITS.maxLineCodeUnits)}`);
  code("too_many_lines", `${"x\n".repeat(LRC_LIMITS.maxLines)}x`);
  code("too_many_timestamps", `${"[00:01.00]".repeat(LRC_LIMITS.maxTimestampsPerLine + 1)}line`);
});

test("empty and metadata-only documents are not accepted as lyrics", () => {
  code("empty_content", "\n \r\n");
  code("empty_content", "[ar:Artist]\n[offset:10]");
});

test("result is deterministic and parsing does not mutate the input", () => {
  const input = "[00:04.00]end\n[00:01.00]start\n[00:01.00]tie";
  const before = input;
  const first = parseLrc(input), second = parseLrc(input);
  assert.deepEqual(first, second);
  assert.deepEqual(first.kind === "synchronized" ? first.events.map(event => event.text) : [], ["start", "tie", "end"]);
  assert.equal(input, before);
});
