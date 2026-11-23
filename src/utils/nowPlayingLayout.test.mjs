import test from "node:test";
import assert from "node:assert/strict";
import { NOW_PLAYING_LAYOUTS, nowPlayingLayout, normalizeQueuePanels, parseNowPlayingLook, readNowPlayingLook, saveNowPlayingLook, saveQueuePanels } from "./nowPlayingLayout.ts";

test("Lyrics is the fifth persisted layout while the four existing identifiers and invalid fallback stay stable", () => {
  assert.deepEqual(NOW_PLAYING_LAYOUTS, ["audiophile", "psycho", "sleeve", "poster", "lyrics"]);
  for (const layout of NOW_PLAYING_LAYOUTS) assert.equal(nowPlayingLayout(layout), layout);
  assert.equal(nowPlayingLayout("unexpected"), "audiophile");
  assert.deepEqual(parseNowPlayingLook('{"layout":"lyrics","psychoPlacement":"background"}'),
    { layout: "lyrics", psychoPlacement: "background" });
  assert.equal(parseNowPlayingLook('{"layout":"removed-layout"}').layout, "audiophile");
});

test("old queue panel preferences gain a default Lyrics entry without changing the four saved choices", () => {
  assert.deepEqual(normalizeQueuePanels({ audiophile: false, psycho: true, sleeve: false, poster: true }),
    { audiophile: false, psycho: true, sleeve: false, poster: true, lyrics: true });
});

test("the fifth layout persists through the existing preference keys and old layout values remain readable", () => {
  const previous = globalThis.window; const stored = new Map();
  globalThis.window = { localStorage: { getItem: key => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value) } };
  try {
    saveNowPlayingLook({ layout: "lyrics", psychoPlacement: "frame" });
    assert.equal(readNowPlayingLook().layout, "lyrics");
    for (const layout of ["audiophile", "psycho", "sleeve", "poster"]) {
      stored.set("sonante.nowPlaying.look.v1", JSON.stringify({ layout }));
      assert.equal(readNowPlayingLook().layout, layout);
    }
    const panels = normalizeQueuePanels({ audiophile: true, psycho: false, sleeve: true, poster: false, lyrics: false });
    saveQueuePanels(panels);
    assert.equal(JSON.parse(stored.get("sonante.nowPlaying.queuePanels.v1")).lyrics, false);
  } finally { globalThis.window = previous; }
});
