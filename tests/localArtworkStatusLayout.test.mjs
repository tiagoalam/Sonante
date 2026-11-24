import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const view = readFileSync(new URL("../src/components/LocalBrowserView.tsx", import.meta.url), "utf8");
const english = JSON.parse(readFileSync(new URL("../src/locales/en.json", import.meta.url), "utf8"));
const portuguese = JSON.parse(readFileSync(new URL("../src/locales/pt-BR.json", import.meta.url), "utf8"));

test("cover progress uses the reserved Local header slot and does not compete with SQLite activity", () => {
  assert.match(view, /const artworkProgress = viewMode === "albums" && \(/);
  assert.match(view, /const artworkProgressVisible = artworkStatus\.active \|\| artworkStatus\.outcome !== "idle"/);
  assert.match(view, /<div role="status" aria-label=\{t\("artwork\.progressLabel"\)\} aria-live=\{artworkProgressVisible \? "polite" : "off"\} aria-atomic="false"/);
  assert.match(view, /!sqliteBacked && \(windowsLibraryStatus === undefined \? artworkProgress/);
  assert.match(view, /windowsLibraryStatus !== undefined/);
  assert.match(view, /grid min-h-\[84px\] shrink-0 grid-cols-\[minmax\(0,1fr\)\]/);
  assert.match(view, /sm:grid-cols-\[minmax\(220px,1fr\)_minmax\(0,1\.8fr\)_auto\]/);
  assert.match(view, /className=\{`ml-auto flex h-9 w-32 shrink-0 flex-col justify-center overflow-hidden sm:w-52\$\{artworkProgressVisible \? "" : " invisible"\}`\}/);
  assert.match(view, /artworkStatus\.total > 0[\s\S]*?artwork\.progressCount/);
  assert.match(view, /Math\.max\(0, artworkStatus\.total - 1\)/);
  assert.match(view, /artworkStatus\.outcome === "partial"[\s\S]*?artwork\.progressPartial/);
  assert.match(view, /artworkStatus\.outcome === "cancelled"[\s\S]*?artwork\.progressCancelled/);
  assert.match(view, /<Search size=\{11\} aria-hidden="true" className="shrink-0 text-\[color:var\(--theme-fg-accent,#E5A00D\)\]" \/>/);
  assert.match(view, /artwork\.progressLabel/);
  assert.match(view, /artworkStatus\.onlineFound > 0/);
  assert.match(view, /text-\[color:var\(--theme-fg-accent,#E5A00D\)\]/);
  assert.match(view, /artworkStatus\.total > 0 && artworkProgressVisible && <div role="progressbar"/);
  assert.match(view, /artworkStatus\.active && viewMode === "albums" \? artworkProgress/);
  assert.doesNotMatch(view, /artwork\.searchingOnline|onlineArtworkSearchStarted|showOnlineArtworkSearch/);
  assert.doesNotMatch(view, /artwork\.found\b/);
});

test("cover progress labels are translated in both locales", () => {
  assert.equal(portuguese.artwork.progressLabel, "Busca de capas");
  assert.equal(english.artwork.progressLabel, "Cover lookup");
  assert.equal(portuguese.artwork.progressCount, "{{processed}} de {{total}} álbuns verificados");
  assert.equal(english.artwork.progressCount, "{{processed}} of {{total}} albums checked");
  assert.ok(portuguese.artwork.progressCompleted && portuguese.artwork.progressPartial && portuguese.artwork.progressCancelled);
  assert.ok(english.artwork.progressCompleted && english.artwork.progressPartial && english.artwork.progressCancelled);
  assert.equal(portuguese.artwork.progressOnline, "{{count}} capas encontradas");
  assert.equal(english.artwork.progressOnline, "Found {{count}} covers");
  assert.ok(portuguese.artwork.progressBarLabel);
  assert.ok(english.artwork.progressBarLabel);
  assert.equal("searchingOnline" in portuguese.artwork, false);
  assert.equal("searchingOnline" in english.artwork, false);
  assert.ok(portuguese.settings.onlineArtworkFailed && portuguese.settings.onlineArtworkPartial && portuguese.settings.onlineArtworkCancelled);
  assert.ok(english.settings.onlineArtworkFailed && english.settings.onlineArtworkPartial && english.settings.onlineArtworkCancelled);
});
