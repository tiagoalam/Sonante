import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("./opraErrors.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
const exports = {};
new Function("exports", code)(exports);

test("OPRA backend codes map to localized categories without exposing raw diagnostics", () => {
  const cases = [
    ["OPRA_QUERY_INVALID", "invalidQuery"],
    ["OPRA_NETWORK_TIMEOUT", "timeout"],
    ["OPRA_NETWORK_UNAVAILABLE", "networkUnavailable"],
    ["OPRA_CATALOG_UNAVAILABLE", "catalogUnavailable"],
    ["OPRA_REDIRECT_REJECTED", "redirectRejected"],
    ["OPRA_CATALOG_INCOMPLETE", "incompleteCatalog"],
    ["OPRA_CATALOG_INVALID_JSONL line=4 kind=MalformedJson", "invalidJsonl"],
    ["OPRA_CATALOG_SCHEMA_UNSUPPORTED line=2 kind=UnknownRecordType", "unsupportedSchema"],
    ["OPRA_CATALOG_SCHEMA_INCOMPATIBLE line=8 kind=InvalidRecord", "incompatibleSchema"],
    ["OPRA_CATALOG_RECORD_INVALID line=2 kind=InvalidIdentifier", "invalidRecord"],
    ["OPRA_CATALOG_INVALID line=12 kind=InvalidJson", "invalidCatalog"],
    ["OPRA_CATALOG_EMPTY", "emptyCatalog"],
    ["OPRA_CATALOG_TOO_LARGE", "catalogTooLarge"],
    ["OPRA_CACHE_UNSAFE", "unsafeCache"],
    ["OPRA_CACHE_UNAVAILABLE", "cacheUnavailable"],
    ["IPC rejected: OPRA_NETWORK_TIMEOUT", "timeout"],
    ["Error: /private/profile/token=secret", "unknown"],
  ];
  for (const [input, expected] of cases) assert.equal(exports.opraLoadError(input), expected);
});

test("every OPRA catalog error category has a localized message in both supported languages", () => {
  const categories = ["timeout", "networkUnavailable", "catalogUnavailable", "redirectRejected", "incompleteCatalog", "invalidJsonl", "unsupportedSchema", "incompatibleSchema", "invalidRecord", "invalidCatalog", "emptyCatalog", "catalogTooLarge", "unsafeCache", "cacheUnavailable", "unknown"];
  for (const language of ["en", "pt-BR"]) {
    const messages = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8")).equalizer.library.opraErrors;
    for (const category of categories) assert.equal(typeof messages[category], "string", `${language}: ${category}`);
  }
});
