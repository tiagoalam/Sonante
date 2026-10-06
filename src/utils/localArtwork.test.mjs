import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { lookupAlbumArtwork } from "./localArtwork.ts";

const album = {
  id: "root|album", title: "Album", artist: "Artist", year: "2000",
  folder_path: "root/CD1", discs: [
    { folder_path: "root/CD1" },
    { folder_path: "root/CD2" },
    { folder_path: "root/CD3" },
  ],
};

test("local artwork wins and online is not called", async () => {
  let online = 0;
  const result = await lookupAlbumArtwork(album, async () => "local", async () => { online++; return "online"; }, true, false);
  assert.equal(result, "local");
  assert.equal(online, 0);
});

test("online waits for all discs and is skipped when disabled or updating", async () => {
  const paths = [];
  let online = 0;
  const local = async (path) => { paths.push(path); return null; };
  const remote = async () => { online++; return "online"; };
  assert.equal(await lookupAlbumArtwork(album, local, remote, false, false), null);
  assert.equal(await lookupAlbumArtwork(album, local, remote, true, true), null);
  assert.equal(online, 0);
  paths.length = 0;
  assert.equal(await lookupAlbumArtwork(album, local, remote, true, false), "online");
  assert.deepEqual(paths, ["root/CD1", "root/CD2", "root/CD3"]);
  assert.equal(online, 1);
});

test("simultaneous online lookup for the same album is deduplicated", async () => {
  let calls = 0;
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const remote = async () => { calls++; return pending; };
  const first = lookupAlbumArtwork(album, async () => null, remote, true, false);
  const second = lookupAlbumArtwork(album, async () => null, remote, true, false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls, 1);
  release("cover");
  assert.deepEqual(await Promise.all([first, second]), ["cover", "cover"]);
});

test("an update beginning during local lookup prevents a new online request", async () => {
  let updating = false;
  let online = 0;
  const result = await lookupAlbumArtwork(album, async () => { updating = true; return null; }, async () => { online++; return "online"; }, true, () => updating);
  assert.equal(result, null);
  assert.equal(online, 0);
});

test("online artwork settings have locale parity", () => {
  for (const locale of ["en", "pt-BR"]) {
    const settings = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8")).settings;
    assert.ok(settings.onlineArtwork);
    assert.ok(settings.onlineArtworkDescription);
  }
});
