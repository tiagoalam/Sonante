import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const app = readFileSync(new URL("../../src/App.tsx", import.meta.url), "utf8");
const ast = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let expression;
function visit(node) {
  if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "clearCollection") expression = node.initializer.getText(ast);
  ts.forEachChild(node, visit);
}
visit(ast);
assert.ok(expression, "real collection reset must remain inspectable");
const compiled = ts.transpileModule(`const reset = (${expression});`, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;

// Supply the real App closure, with observable state seeded stale. Wrapping
// existing setters preserves each caller's navigation/inventory assertions.
export function bindCollectionReset(scope) {
  const state = { active: { rating_key: "stale-collection" }, albums: [{ rating_key: "stale-album" }], loading: true };
  const generation = scope.collectionGeneration ?? { current: 7 };
  scope.collectionGeneration = generation;
  const initial = generation.current;
  for (const [setter, key] of [["setActiveCollection", "active"], ["setCollectionAlbums", "albums"], ["setLoadingCollection", "loading"]]) {
    const previous = scope[setter];
    scope[setter] = value => { state[key] = value; previous?.(value); };
  }
  scope.clearCollection = new Function(...Object.keys(scope), `${compiled}; return reset;`)(...Object.values(scope));
  return { state, generation, assertReset(times = 1) {
    assert.equal(generation.current, initial + times);
    assert.equal(state.active, null);
    assert.deepEqual(state.albums, []);
    assert.equal(state.loading, false);
  } };
}
