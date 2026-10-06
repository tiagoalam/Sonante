import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initialMediaSource } from "./appBootstrap.ts";

test("Local Browser has no source before configuration resolves", () => {
  assert.equal(initialMediaSource(null), null);
  assert.equal(initialMediaSource({ plex_token: "" }), "local");
  assert.equal(initialMediaSource({ plex_token: " token " }), "plex");
});

test("App gates LocalBrowserView behind initial config and devices", () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.ok(source.includes("Promise.all([configService.getConfig(), configService.getAudioDevices()])"));
  assert.ok(source.indexOf("if (!config || mediaSource === null)") < source.indexOf("<LocalBrowserView"));
});
