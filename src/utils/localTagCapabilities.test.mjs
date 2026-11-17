import test from "node:test";
import assert from "node:assert/strict";
import ts from "typescript";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("./localTagCapabilities.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const url = `data:text/javascript;base64,${Buffer.from(js).toString("base64")}`;
const { localTagEditingAvailable } = await import(url);

const ready = {
  sqliteBacked: false,
  detailUnavailable: false,
  catalogLoaded: true,
  catalogLoading: false,
  catalogError: false,
  mpdBusy: false,
  saveBlocked: false,
  manualCheckPlayback: false,
  libraryUpdating: false,
  albumPresent: true,
};

test("SQLite album editor requires explicit backend support", () => {
  assert.equal(localTagEditingAvailable({ ...ready, sqliteBacked: true }), false);
  assert.equal(localTagEditingAvailable({ ...ready, sqliteBacked: true, sqliteEditSupported: true }), false);
  assert.equal(localTagEditingAvailable({ ...ready, sqliteBacked: true, sqliteEditSupported: true, earlyAccessEnabled: true }), true);
  assert.equal(localTagEditingAvailable({ ...ready, sqliteBacked: true, sqliteEditSupported: true, earlyAccessEnabled: false }), false);
});

test("original Local tag editing still follows its existing MPD catalog guards", () => {
  assert.equal(localTagEditingAvailable(ready), true);
  for (const override of [
    { detailUnavailable: true }, { catalogLoaded: false }, { catalogLoading: true },
    { catalogError: true }, { mpdBusy: true }, { saveBlocked: true },
    { manualCheckPlayback: true }, { libraryUpdating: true }, { albumPresent: false },
  ]) assert.equal(localTagEditingAvailable({ ...ready, ...override }), false);
});
