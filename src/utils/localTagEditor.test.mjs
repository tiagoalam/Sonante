import test from "node:test";
import assert from "node:assert/strict";
import {
  analyzeLocalTagForm,
  createLocalTagEditorDraft,
  localTagSaveOutcome,
  markLocalTagRevisionConflict,
  parsePositiveTagInteger,
} from "./localTagEditor.ts";

const tags = (overrides = {}) => ({
  title: "Original",
  artist: null,
  album: "Album",
  album_artist: null,
  genre: null,
  date: null,
  track_number: 2,
  track_total: 10,
  disc_number: 1,
  disc_total: 2,
  format: "flac",
  readable: true,
  writable: true,
  editable: true,
  unsupported_reason: null,
  revision: "v1.original",
  ...overrides,
});

test("unchanged fields produce unchanged patch actions and clean initial state", () => {
  const draft = createLocalTagEditorDraft(tags());
  const analysis = analyzeLocalTagForm(draft.initial, draft.values);
  assert.equal(analysis.dirty, false);
  assert.equal(analysis.valid, true);
  assert.ok(Object.values(analysis.patch).every((item) => item.action === "unchanged"));
});

test("new values use set while clearing an existing value uses remove", () => {
  const draft = createLocalTagEditorDraft(tags());
  const analysis = analyzeLocalTagForm(draft.initial, {
    ...draft.values,
    title: "Changed",
    album: "",
  });
  assert.deepEqual(analysis.patch.title, { action: "set", value: "Changed" });
  assert.deepEqual(analysis.patch.album, { action: "remove" });
  assert.equal(analysis.dirty, true);
});

test("an absent field left visually empty remains unchanged", () => {
  const draft = createLocalTagEditorDraft(tags({ artist: null }));
  assert.deepEqual(
    analyzeLocalTagForm(draft.initial, draft.values).patch.artist,
    { action: "unchanged" },
  );
});

test("track and disc values accept only positive safe integers", () => {
  assert.equal(parsePositiveTagInteger("1"), 1);
  assert.equal(parsePositiveTagInteger("01"), null);
  assert.equal(parsePositiveTagInteger("0"), null);
  assert.equal(parsePositiveTagInteger("-2"), null);
  assert.equal(parsePositiveTagInteger("1.5"), null);
  assert.equal(parsePositiveTagInteger("999999999999999999999"), null);
});

test("totals smaller than their number are invalid", () => {
  const draft = createLocalTagEditorDraft(tags());
  const analysis = analyzeLocalTagForm(draft.initial, {
    ...draft.values,
    track_number: "8",
    track_total: "7",
    disc_number: "3",
    disc_total: "2",
  });
  assert.equal(analysis.valid, false);
  assert.equal(analysis.errors.track_total, "total_less_than_number");
  assert.equal(analysis.errors.disc_total, "total_less_than_number");
});

test("committed MPD failure is a saved warning, never a save failure", () => {
  const base = { tags: tags(), mpd_refresh: { requested: true, job_id: null, error_code: null } };
  assert.equal(localTagSaveOutcome({ ...base, committed: true, mpd_refresh: { ...base.mpd_refresh, success: true } }), "saved");
  assert.equal(localTagSaveOutcome({ ...base, committed: true, mpd_refresh: { ...base.mpd_refresh, success: false } }), "saved_refresh_failed");
  assert.equal(localTagSaveOutcome({ ...base, committed: false, mpd_refresh: { ...base.mpd_refresh, success: false } }), "not_committed");
});

test("revision conflict preserves edits until a reloaded draft replaces them", () => {
  const draft = createLocalTagEditorDraft(tags());
  const edited = { ...draft, values: { ...draft.values, title: "My edit" } };
  const conflicted = markLocalTagRevisionConflict(edited);
  assert.equal(conflicted.values.title, "My edit");
  assert.equal(conflicted.revisionConflict, true);

  const reloaded = createLocalTagEditorDraft(tags({ title: "External edit", revision: "v1.new" }));
  assert.equal(reloaded.values.title, "External edit");
  assert.equal(reloaded.initial.revision, "v1.new");
  assert.equal(reloaded.revisionConflict, false);
});
