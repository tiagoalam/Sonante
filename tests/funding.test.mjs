import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

test("GitHub Sponsors funding file uses valid GitHub funding YAML with only the configured account", () => {
  const source = readFileSync(new URL("../.github/FUNDING.yml", import.meta.url), "utf8");
  const match = source.match(/^github:\s*\[([a-z0-9-]+)\]\s*$/u);
  assert.ok(match, "the file must be a valid simple YAML mapping to a flow sequence");
  assert.deepEqual(match[1].split(",").map(value => value.trim()), ["tiagoalam"]);
});
