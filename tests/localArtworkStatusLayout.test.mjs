import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const view = readFileSync(new URL("../src/components/LocalBrowserView.tsx", import.meta.url), "utf8");
const english = JSON.parse(readFileSync(new URL("../src/locales/en.json", import.meta.url), "utf8"));
const portuguese = JSON.parse(readFileSync(new URL("../src/locales/pt-BR.json", import.meta.url), "utf8"));

test("album progress stays reserved in the header and has a stable compact size", () => {
  assert.match(view, /\{viewMode === "albums" && \(\s*<div role="status" aria-live=\{artworkStatus\.active \? "polite" : "off"\} aria-atomic="false"/);
  assert.match(view, /<div className="flex flex-wrap items-center justify-between gap-4 px-8 pt-8 pb-4">\s*<div className="flex min-w-0 flex-1 items-center space-x-3">/);
  assert.match(view, /className=\{`ml-auto flex h-9 w-32 shrink-0 flex-col justify-center overflow-hidden sm:w-52\$\{artworkStatus\.active \? "" : " invisible"\}`\}/);
  assert.doesNotMatch(view, /artworkStatus\.active\s*&&\s*\(\s*<div/);
  assert.match(view, /<Search size=\{11\} aria-hidden="true" className="shrink-0 text-\[color:var\(--theme-fg-accent,#E5A00D\)\]" \/>/);
  assert.match(view, /artwork\.progressLabel/);
  assert.match(view, /Math\.min\(artworkStatus\.processed, artworkStatus\.total\)\}\/{artworkStatus\.total\}/);
  assert.match(view, /artworkStatus\.onlineFound > 0/);
  assert.match(view, /text-\[color:var\(--theme-fg-accent,#E5A00D\)\]/);
  assert.match(view, /artworkStatus\.total > 0 && <div role="progressbar"/);
  assert.doesNotMatch(view, /artwork\.searchingOnline|onlineArtworkSearchStarted|showOnlineArtworkSearch/);
  assert.doesNotMatch(view, /artwork\.found\b/);
});

test("cover progress labels are translated in both locales", () => {
  assert.equal(portuguese.artwork.progressLabel, "Capas online");
  assert.equal(english.artwork.progressLabel, "Online covers");
  assert.equal(portuguese.artwork.progressOnline, "Encontradas {{count}}");
  assert.equal(english.artwork.progressOnline, "Found {{count}}");
  assert.ok(portuguese.artwork.progressBarLabel);
  assert.ok(english.artwork.progressBarLabel);
  assert.equal("searchingOnline" in portuguese.artwork, false);
  assert.equal("searchingOnline" in english.artwork, false);
});
