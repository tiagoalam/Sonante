import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const newStyleIds = ["tone_classic_v3", "tone_dance_v3", "tone_hip_hop_v3", "tone_rnb_v3"];
const originalStyleIds = ["style_rock_v2", "style_reggae_v2", "style_jazz_v2", "style_electronic_v2", "style_pop_v2", "style_acoustic_v2"];
const retiredDuplicateIds = ["tone_pop_v3", "tone_rock_v3", "tone_jazz_v3"];
const retiredK17Ids = [
  "k17_classic_ref_v1", "k17_dance_ref_v1", "k17_pop_ref_v1", "k17_hip_hop_ref_v1",
  "k17_rnb_ref_v1", "k17_rock_ref_v1", "k17_jazz_ref_v1",
];

test("EQ keeps unique rhythms in Styles and exposes no ten-band category or device-branded preset text", () => {
  for (const locale of ["en", "pt-BR"]) {
    const equalizer = JSON.parse(readFileSync(new URL(`../src/locales/${locale}.json`, import.meta.url), "utf8")).equalizer;
    const visible = [
      ...Object.values(equalizer.builtin),
      ...Object.values(equalizer.descriptions),
      ...Object.values(equalizer.groups),
    ];
    assert.ok(visible.every(value => typeof value === "string" && !/k17|fiio/i.test(value)), `${locale} visible preset text is generic`);
    assert.equal(equalizer.groups.tones, undefined, `${locale} has no separate ten-band group`);
    assert.ok(equalizer.groups.styles, `${locale} keeps the existing Styles category`);

    const styleNames = [...originalStyleIds, ...newStyleIds].map(id => {
      assert.ok(equalizer.builtin[id], `${locale} label exists for ${id}`);
      assert.ok(equalizer.descriptions[id], `${locale} description exists for ${id}`);
      return equalizer.builtin[id].toLocaleLowerCase(locale);
    });
    assert.equal(new Set(styleNames).size, styleNames.length, `${locale} normal style labels are unique`);
    for (const id of newStyleIds) assert.ok(!/10 bands|10 bandas/i.test(equalizer.builtin[id]));

    for (const id of retiredDuplicateIds) {
      assert.equal(equalizer.builtin[id], equalizer.builtin[retiredDuplicateIds[0]], `${locale} does not expose a duplicate Rock/Pop/Jazz label`);
      assert.match(equalizer.descriptions[id], /retired|retirada/i);
    }
    for (const id of retiredK17Ids) {
      assert.ok(equalizer.builtin[id] && equalizer.descriptions[id], `${locale} can explain a retained saved reference`);
      assert.match(equalizer.descriptions[id], /retained unchanged|mantida sem alterações/i);
    }
  }
});
