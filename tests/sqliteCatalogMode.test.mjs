import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("../src/utils/sqliteExperimentalMode.ts", import.meta.url), "utf8");
const start = source.indexOf("export function sqliteCatalogEnabledFor");
const end = source.indexOf("\n}\n\nexport function", start) + 2;
const functionSource = source.slice(start, end);
const compiled = ts.transpileModule(functionSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;
const exports = {};
new Function("exports", compiled)(exports);

test("SQLite is the default catalog in production and in development", () => {
  assert.equal(exports.sqliteCatalogEnabledFor(true, false, ""), true);
  assert.equal(exports.sqliteCatalogEnabledFor(true, true, ""), true);
  assert.equal(exports.sqliteCatalogEnabledFor(true, true, "?sqlite-experimental=1"), true);
});

test("legacy MPD catalog is only an explicit development opt-out", () => {
  assert.equal(exports.sqliteCatalogEnabledFor(true, true, "?local-catalog=mpd"), false);
  assert.equal(exports.sqliteCatalogEnabledFor(true, false, "?local-catalog=mpd"), true);
  assert.equal(exports.sqliteCatalogEnabledFor(false, false, ""), false);
});
