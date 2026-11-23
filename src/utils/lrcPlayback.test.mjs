import test from "node:test";
import assert from "node:assert/strict";
import { parseLrc } from "./lrcParser.ts";
import { activeLrcEventIndex } from "./lrcPlayback.ts";

test("playback position selects no line before the first timestamp and the latest line thereafter", () => {
  const parsed = parseLrc("[offset:250]\n[00:01.00]one\n[00:02.50]two\n[00:04.00]three");
  assert.deepEqual([0, 999, 1_000, 2_499, 2_500, 9_000].map(position => activeLrcEventIndex(parsed, position)),
    [null, null, 0, 0, 1, 2]);
  assert.equal(parsed.metadata.offsetMs, 250, "offset remains preserved metadata and is not applied to the player clock");
});

test("negative offsets are not applied twice, and seeks in both directions use the supplied snapshot", () => {
  const parsed = parseLrc("[offset:-375]\n[00:01.00]one\n[00:02.00]two\n[00:03.00]three");
  assert.equal(activeLrcEventIndex(parsed, 2_250), 1);
  assert.equal(activeLrcEventIndex(parsed, 500), null, "backward seek returns before the first line");
  assert.equal(activeLrcEventIndex(parsed, 3_000), 2, "forward seek immediately selects the current line");
  assert.equal(activeLrcEventIndex(parsed, 1_500), 0, "a later backward seek selects the earlier line");
  assert.equal(parsed.metadata.offsetMs, -375);
});

test("pause and resume at the same player position keep the same line; progress changes only with a newer snapshot", () => {
  const parsed = parseLrc("[00:01.00]one\n[00:02.00]two");
  const pausedPosition = 1_600;
  assert.equal(activeLrcEventIndex(parsed, pausedPosition), 0);
  assert.equal(activeLrcEventIndex(parsed, pausedPosition), 0, "no independent clock advances while paused");
  const resumedPosition = 2_100;
  assert.equal(activeLrcEventIndex(parsed, resumedPosition), 1, "resume follows the next confirmed player snapshot");
});

test("stable duplicate timestamps select the last event at that time; plain text has no active line", () => {
  const synced = parseLrc("[00:01.00]first\n[00:01.00]second");
  assert.equal(activeLrcEventIndex(synced, 1_000), 1);
  assert.equal(activeLrcEventIndex(parseLrc("unsynchronized\ntext"), 10_000), null);
  assert.equal(activeLrcEventIndex(synced, -1), null);
  assert.equal(activeLrcEventIndex(synced, 0.5), null);
});
