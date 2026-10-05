import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { albumLocationSources } from "./localAlbumLocations.ts";

const album = (discs) => ({ folder_path: "Album/CD1", discs });
const disc = (number, label) => ({ number, label, folder_path: `Album/${label}`, track_count: 1 });

test("single disc uses the album path without an artificial label", () => {
  assert.deepEqual(albumLocationSources(album([disc(1, "CD1")])), [
    { label: null, path: "Album/CD1" },
  ]);
});

test("multiple discs preserve all labels and paths in order", () => {
  const discs = [disc(1, "CD 1"), disc(2, "CD 2 (Dub Wise)"), disc(3, "Disc 3")];
  assert.deepEqual(albumLocationSources(album(discs)), discs.map((item) => ({
    label: item.label,
    path: item.folder_path,
  })));
});

test("location label exists in both locales", () => {
  for (const locale of ["en", "pt-BR"]) {
    const content = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    assert.ok(content.localBrowser.location);
  }
});
