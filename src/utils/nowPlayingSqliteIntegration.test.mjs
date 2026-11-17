import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { NowPlayingHosting } from "./nowPlayingHosting.ts";

test("SQLite mode mounts the original embedded Now Playing host after the user opens it", async () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /const nowPlayingEmbedded = hosting\.mode === "embedded"/);
  assert.match(app, /\{nowPlayingEmbedded && <div[^]*?<NowPlayingHost snapshot=\{\{ usageMode:[^]*?onNavigate=\{view => void navigateFromNowPlaying\(view\)\}/);
  assert.match(app, /onOpenNowPlaying=\{showNowPlaying\}/);
  const readyStart = app.indexOf("listen(NOW_PLAYING_READY_EVENT");
  const readyEnd = app.indexOf("LOCAL_LIBRARY_RECONCILED_EVENT", readyStart);
  const readyEffect = app.slice(readyStart, readyEnd);
  assert.ok(readyEffect.includes("publishCurrentNowPlayingSnapshot()"));
  assert.doesNotMatch(readyEffect, /sqlite-experimental/);

  let notify;
  const coordinator = new NowPlayingHosting({
    claim: async (host, sessionId) => ({ host, sessionId, generation: 1 }),
    open: async () => "runtime", activate: async () => {}, suspend: async () => {}, close: async () => {},
    stop: async () => {}, id: () => "sqlite-session",
  });
  coordinator.subscribe(() => {
    const state = coordinator.getSnapshot();
    if (state.mode === "embedded" && state.owner) queueMicrotask(() => notify?.(state.owner));
  });
  notify = owner => coordinator.hostReady(owner);
  await coordinator.show();
  assert.equal(coordinator.getSnapshot().mode, "embedded");
  assert.equal(coordinator.getSnapshot().owner.host, "main");
});

test("Now Playing host receives current playback and source information rather than an MPD catalog lookup", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const embedded = app.slice(app.indexOf("{nowPlayingEmbedded && <div"), app.indexOf("data-now-playing-player-bar"));
  assert.match(embedded, /playback: presentationPlayback/);
  assert.match(embedded, /sourceInfo/);
  assert.match(embedded, /queueArtwork/);
  assert.match(app, /const sourceInfo = usePlayerSourceInfo\(playbackStatus\.current_media, sourceInfoContext/);
});
