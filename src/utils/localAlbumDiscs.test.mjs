import assert from "node:assert/strict";
import test from "node:test";
import { flattenAlbumDiscs } from "./localAlbumDiscs.ts";

test("flattens discs in order and gives CD2 a global start index", () => {
  const file = (name) => ({ item_type: "file", path: name, name });
  const groups = [
    { disc: { number: 1, label: "CD1", folder_path: "Album/CD1", track_count: 2 }, tracks: [file("A"), file("B")] },
    { disc: { number: 2, label: "CD2", folder_path: "Album/CD2", track_count: 2 }, tracks: [file("C"), file("D")] },
  ];
  const result = flattenAlbumDiscs(groups);
  assert.deepEqual(result.tracks.map((track) => track.name), ["A", "B", "C", "D"]);
  assert.equal(result.sections[1].startIndex, 2);
  assert.equal(result.sections[1].startIndex + 0, 2);
});
