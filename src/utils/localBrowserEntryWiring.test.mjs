import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const source = path => readFileSync(new URL(path, import.meta.url), "utf8");

test("all Local album detail routes use the required catalog-context entry", () => {
  const app = source("../App.tsx");
  const history = source("../components/HistoryView.tsx");
  const editions = source("../components/AlbumEditionsView.tsx");
  const search = source("../components/UnifiedSearchView.tsx");

  assert.match(app, /const localBrowserCatalogContext: LocalBrowserCatalogContext = \{[\s\S]*?sqliteBacked: sqliteMode,[\s\S]*?sqliteSources,[\s\S]*?onSqliteVerifySource: verifySqliteSource/);
  assert.equal((app.match(/<LocalBrowserEntry catalogContext=\{localBrowserCatalogContext\}/g) ?? []).length, 3,
    "main Local, player navigation, and Explore detail share the Local catalog state");
  assert.match(app, /localBrowserCatalogContext=\{localBrowserCatalogContext\}/g);
  assert.match(history, /<LocalBrowserEntry catalogContext=\{localBrowserCatalogContext\} initialAlbumId=\{currentDetail\.local\}/);
  assert.match(editions, /<LocalBrowserEntry catalogContext=\{localBrowserCatalogContext\} initialAlbumId=\{detail\.album\.id\}/);
  assert.match(search, /<LocalBrowserEntry[\s\S]*?catalogContext=\{\{[\s\S]*?sqliteBacked,[\s\S]*?sqliteSources/);
  assert.doesNotMatch(`${app}\n${history}\n${editions}\n${search}`, /<LocalBrowserView\b/,
    "route callers must not bypass the context-enforcing entry");
});
