import test from "node:test";
import assert from "node:assert/strict";
import { audioRestartExpected } from "./audioConfigChange.ts";

const original = {
  first_run: false,
  alsa_device: "default",
  audio_output_type: "pipewire",
  local_folders: ["/music"],
  plex_url: "",
  plex_token: "",
  playback_mode: "http",
  local_mount_path: "",
  remote_share_path: "",
  dop_enabled: false,
  audio_buffer_size_kb: 4096,
  replay_gain: "off",
};

test("library and non-audio preferences do not imply an audio restart", () => {
  assert.equal(audioRestartExpected(original, { ...original, local_folders: ["/new"] }), false);
  assert.equal(audioRestartExpected(original, { ...original, plex_url: "http://plex" }), false);
});

test("each backend audio field implies an audio restart", () => {
  for (const change of [
    { audio_output_type: "alsa" },
    { alsa_device: "hw:1" },
    { dop_enabled: true },
    { audio_buffer_size_kb: 8192 },
    { replay_gain: "track" },
  ]) {
    assert.equal(audioRestartExpected(original, { ...original, ...change }), true);
  }
});
