import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const localeNames = ["en", "pt-BR", "es"];

const loadLocale = (locale) => {
  try {
    const file = new URL(`../src/locales/${locale}.json`, import.meta.url);
    return JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new Error(`${locale}: could not load or parse locale JSON: ${error.message}`, {
      cause: error,
    });
  }
};

const valueType = (value) => {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  return typeof value;
};

const placeholders = (value) =>
  [...new Set([...value.matchAll(/{{\s*([^{}]+?)\s*}}/g)].map((match) => match[1].split(",")[0].trim()))]
    .sort();

const compareLocale = (reference, actual, locale, path, problems) => {
  const expectedType = valueType(reference);
  const actualType = valueType(actual);

  if (expectedType !== actualType) {
    problems.push(`${locale} — ${path || "<root>"}: expected ${expectedType}, found ${actualType}`);
    return;
  }

  if (expectedType === "object" || expectedType === "array") {
    for (const key of Object.keys(reference)) {
      const keyPath = path ? `${path}.${key}` : key;
      if (!Object.hasOwn(actual, key)) {
        problems.push(`${locale} — ${keyPath}: missing key`);
      } else {
        compareLocale(reference[key], actual[key], locale, keyPath, problems);
      }
    }
    for (const key of Object.keys(actual)) {
      if (!Object.hasOwn(reference, key)) {
        problems.push(`${locale} — ${path ? `${path}.` : ""}${key}: extra key`);
      }
    }
    return;
  }

  if (expectedType === "string") {
    if (actual.trim().length === 0) {
      problems.push(`${locale} — ${path}: empty string`);
    }
    const expected = placeholders(reference);
    const found = placeholders(actual);
    if (expected.join("\0") !== found.join("\0")) {
      problems.push(
        `${locale} — ${path}: placeholders differ (expected ${JSON.stringify(expected)}, found ${JSON.stringify(found)})`,
      );
    }
  }
};

test("locales have the same keys, types, and i18next placeholders as en", () => {
  const locales = Object.fromEntries(localeNames.map((locale) => [locale, loadLocale(locale)]));
  const problems = [];

  for (const locale of localeNames) {
    compareLocale(locales.en, locales[locale], locale, "", problems);
  }

  assert.equal(problems.length, 0, problems.join("\n"));
});
