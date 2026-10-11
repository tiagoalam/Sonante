import test from "node:test";
import assert from "node:assert/strict";
import { audioRestartExpected, localFoldersChanged, localFoldersOnlyChanged } from "./audioConfigChange.ts";

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

test("local folder changes are detected without treating an unchanged list as changed", () => {
  assert.equal(localFoldersChanged(original, { ...original, local_folders: ["/music"] }), false);
  assert.equal(localFoldersChanged(original, { ...original, local_folders: ["/music", "/archive"] }), true);
  assert.equal(localFoldersChanged(original, { ...original, local_folders: ["/archive"] }), true);
  assert.equal(localFoldersChanged(original, { ...original, local_folders: [] }), true);
});

test("background Save is limited to local-folder-only changes", () => {
  assert.equal(localFoldersOnlyChanged(original, { ...original, local_folders: ["/new"] }), true);
  assert.equal(localFoldersOnlyChanged(original, original), false);
  assert.equal(localFoldersOnlyChanged(original, { ...original, local_folders: ["/new"], alsa_device: "hw:1" }), false);
  assert.equal(localFoldersOnlyChanged(original, { ...original, local_folders: ["/new"], plex_url: "http://plex" }), false);
  assert.equal(localFoldersOnlyChanged(original, { ...original, local_folders: ["/new"], online_artwork_enabled: false }), false);
  assert.equal(localFoldersOnlyChanged({ ...original, first_run: true }, { ...original, first_run: true, local_folders: ["/new"] }), false);
});
