import test from "node:test";
import assert from "node:assert/strict";
import { equalizerErrorCode } from "./equalizerError.ts";
import { readFileSync } from "node:fs";

test("EQ diagnostics preserve stable codes while redacting arbitrary error details", () => {
  assert.equal(equalizerErrorCode("EQ_RuntimeStartFailed; private path /home/user/audio"), "EQ_RuntimeStartFailed");
  assert.equal(equalizerErrorCode(new Error("EQ_RuntimeStartFailed")), "EQ_RuntimeStartFailed");
  assert.equal(equalizerErrorCode({ code: "EQ_RuntimeStartFailed", message: "/home/user/audio" }), "EQ_RuntimeStartFailed");
  assert.equal(equalizerErrorCode({ message: "EQ_MPD_RUNTIME_UNAVAILABLE; /home/user/audio" }), "EQ_MPD_RUNTIME_UNAVAILABLE");
  assert.equal(equalizerErrorCode("continuity_lost"), "continuity_lost");
  assert.equal(equalizerErrorCode("/home/user/audio: permission denied"), "EQ_UNCONFIRMED");
  assert.equal(equalizerErrorCode(new Error("secret path")), "EQ_UNCONFIRMED");
  assert.equal(equalizerErrorCode({ code: "/home/user/audio", message: "secret path" }), "EQ_UNCONFIRMED");
});

test("EqualizerView exposes the sanitized operation/runtime diagnostic", () => {
  const source = readFileSync(new URL("../components/EqualizerView.tsx", import.meta.url), "utf8");
  assert.match(source, /equalizerErrorCode\(reason\)/);
  assert.match(source, /data-eq-diagnostic/);
  assert.match(source, /error\?\?current\?\.error/);
  assert.match(source, /data-eq-runtime-diagnostic/);
});
