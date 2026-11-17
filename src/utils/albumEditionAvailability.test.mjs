import test from "node:test";
import assert from "node:assert/strict";
import ts from "typescript";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("./albumEditionAvailability.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const url = `data:text/javascript;base64,${Buffer.from(js).toString("base64")}`;
const { activateEditionAvailability, publishVerifiedEditionAvailability, hasVerifiedEditionAlternative, shouldShowAlbumEditionsAction } = await import(url);

test("album actions require the experimental flag, a valid album, and a verified alternative", () => {
  assert.equal(shouldShowAlbumEditionsAction(false, true, true), false);
  assert.equal(shouldShowAlbumEditionsAction(true, false, true), false);
  assert.equal(shouldShowAlbumEditionsAction(true, true, false), false);
  assert.equal(shouldShowAlbumEditionsAction(true, true, true), true);
});

test("only current verified edition identities enable the action, scoped by source", () => {
  const context = {};
  activateEditionAvailability("local", context);
  assert.equal(hasVerifiedEditionAlternative("local", "album-1"), false);
  publishVerifiedEditionAvailability("local", context, ["album-1"]);
  assert.equal(hasVerifiedEditionAlternative("local", "album-1"), true);
  assert.equal(hasVerifiedEditionAlternative("plex", "album-1"), false);

  const refreshedContext = {};
  activateEditionAvailability("local", refreshedContext);
  assert.equal(hasVerifiedEditionAlternative("local", "album-1"), false);
  publishVerifiedEditionAvailability("local", context, ["stale-album"]);
  assert.equal(hasVerifiedEditionAlternative("local", "stale-album"), false);
});
