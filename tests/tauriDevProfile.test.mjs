import test from "node:test";
import assert from "node:assert/strict";
import { tauriArgsFor } from "../scripts/tauri-dev.mjs";

test("Tauri development on Linux and Windows preserves the isolated dev064 identity", () => {
  assert.deepEqual(tauriArgsFor(["dev"], "linux", "/checkout"), [
    "dev", "--config", "/checkout/src-tauri/tauri.dev.conf.json",
  ]);
  assert.deepEqual(tauriArgsFor(["dev"], "win32", "C:/checkout"), [
    "dev", "--config", "C:/checkout/src-tauri/tauri.dev.conf.json",
  ]);
});

test("production builds keep the official application identity", () => {
  assert.deepEqual(tauriArgsFor(["build"], "linux", "/checkout"), ["build"]);
});
