import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import { parseLrc } from "../src/utils/lrcParser.ts";
import { activeLrcEventIndex } from "../src/utils/lrcPlayback.ts";
import { automaticLyricsMatches, hasSupportedLyricsIdentity, lyricsKey, lyricsQuery, LYRICS_ENABLED_KEY, readLyricsOnlinePreference } from "../src/utils/trackLyrics.ts";

const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async () => { for (let index = 0; index < 20; index++) await Promise.resolve(); };
const media = (uri, queue_index = 2) => ({ kind: "local", uri, queue_index });
function harness(audioService, lookup = async () => ({ kind: "not_found" })) {
  let cursor = 0, disposed = false, lateWrites = 0, scrolls = 0; const scrollRequests = [], slots = [], effects = [];
  const hooks = { ...React, default: React,
    useState(initial) { const index = cursor++; if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial; return [slots[index], value => { if (disposed) lateWrites++; slots[index] = typeof value === "function" ? value(slots[index]) : value; }]; },
    useRef(initial) { const index = cursor++; return slots[index] ??= { current: initial }; },
    useEffect(fn, deps) { const index = cursor++, previous = slots[index]; if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) effects.push(() => { previous?.cleanup?.(); slots[index] = { deps, cleanup: fn() }; }); },
  };
  const source = readFileSync(new URL("../src/components/TrackLyrics.tsx", import.meta.url), "utf8");
  const js = ts.transpileModule(source, { fileName: "TrackLyrics.tsx", compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(name => ({ react: hooks, "react/jsx-runtime": jsx,
    "react-i18next": { useTranslation: () => ({ t: key => key }) }, "../services/audio": { audioService: { getLyricsChoice: async () => null,
      setLyricsChoice: async () => {}, ...audioService, searchOnlineLyrics: (query, refresh) => lookup(query, undefined, refresh) } },
    "../utils/playerNavigation": { currentMediaKey: item => item ? JSON.stringify(item) : "none" },
    "../utils/lrcParser": { parseLrc }, "../utils/lrcPlayback": { activeLrcEventIndex },
    "../utils/trackLyrics": { automaticLyricsMatches, hasSupportedLyricsIdentity, lyricsKey, lyricsQuery, LYRICS_ENABLED_KEY, LYRICS_PREFERENCE_EVENT: "lyrics-pref", readLyricsOnlinePreference,
      writeLyricsOnlinePreference() {}, lyricsFailureReason: error => error.reason ?? "offline",
      LyricsClient: class { constructor(request) { this.request = request; } lookup(query, signal, refresh) { audioService.onLyricsLookup?.(signal); return this.request(query, signal, refresh); } } },
  })[name], exports);
  return { TrackLyrics: exports.TrackLyrics, render(props) { cursor = 0; const tree = exports.TrackLyrics(props);
    const activeLine = { getBoundingClientRect: () => ({ top: 190, height: 20 }) };
    const container = { scrollTop: 0, clientHeight: 100, contains: element => element === activeLine,
      getBoundingClientRect: () => ({ top: 100 }), scrollTo(options) { scrolls++; scrollRequests.push(options); this.scrollTop = options.top; } };
    nodes(tree, node => node.type === "div" && node.props?.className?.includes("np-lyrics-synchronized") && node.props?.ref)
      .forEach(node => { node.props.ref.current = container; });
    nodes(tree, node => node.type === "p" && node.props?.className?.includes("np-lyrics-line") && node.props?.ref)
      .forEach(node => { node.props.ref.current = node.props["aria-current"] ? activeLine : { getBoundingClientRect: () => ({ top: 190, height: 20 }) }; });
    effects.splice(0).forEach(run => run()); return tree;
  }, dispose() { disposed = true; slots.forEach(slot => slot?.cleanup?.()); }, lateWrites: () => lateWrites, scrolls: () => scrolls, scrollRequests: () => scrollRequests };
}
function nodes(tree, predicate, result = []) { if (!tree || typeof tree !== "object") return result; if (Array.isArray(tree)) { tree.forEach(item => nodes(item, predicate, result)); return result; } if (predicate(tree)) result.push(tree); nodes(tree.props?.children, predicate, result); return result; }
function text(tree) { if (tree == null || typeof tree === "boolean") return ""; if (typeof tree === "string" || typeof tree === "number") return String(tree); if (Array.isArray(tree)) return tree.map(text).join(" "); return text(tree.props?.children); }

test("local lyrics use the exact media identity, do not refetch on playback position, and ignore a late A response after switching to B", async () => {
  const jobs = [];
  const ui = harness({ getCurrentLocalLyrics(identity) { const job = { identity, ...deferred() }; jobs.push(job); return job.promise; } });
  try {
    const trackA = { current_media: media("sqlite-track://source-a/track-a"), elapsed: 1, state: "play" };
    let tree = ui.render({ track: trackA });
    assert.equal(jobs.length, 1); assert.deepEqual(jobs[0].identity, trackA.current_media);
    assert.match(text(tree), /loading/);
    tree = ui.render({ track: { ...trackA, elapsed: 2.2, state: "pause" } });
    assert.equal(jobs.length, 1, "elapsed and pause updates must not request the file again");

    const trackB = { current_media: media("sqlite-track://source-b/track-b"), elapsed: 0, state: "play" };
    tree = ui.render({ track: trackB }); assert.equal(jobs.length, 2);
    jobs[0].resolve({ kind: "found", content: "[00:01.00]stale A" }); await flush();
    tree = ui.render({ track: trackB }); assert.doesNotMatch(text(tree), /stale A/);
    jobs[1].resolve({ kind: "found", content: "[offset:250]\n[00:01.00]B first\n[00:03.00]B second" }); await flush();
    tree = ui.render({ track: { ...trackB, elapsed: 3.1 } });
    const lines = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.deepEqual(lines.map(text), ["B first", "B second"]);
    assert.equal(lines[1].props["aria-current"], "true", "current playback snapshot selects the active line");
    assert.equal(ui.scrolls(), 1, "the active line is scrolled into view from the playback snapshot");
    tree = ui.render({ track: { ...trackB, elapsed: 1.5, state: "pause" } });
    const paused = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.equal(paused[0].props["aria-current"], "true", "pause retains the line for the paused position");
    assert.equal(ui.scrolls(), 2);
    tree = ui.render({ track: { ...trackB, elapsed: 3.5, state: "pause" } });
    const seekForward = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.equal(seekForward[1].props["aria-current"], "true", "forward seek selects from the new snapshot");
    assert.equal(ui.scrolls(), 3);
    tree = ui.render({ track: { ...trackB, elapsed: 0.5, state: "pause" } });
    const rewound = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.equal(rewound.filter(line => line.props["aria-current"]).length, 0, "seek before first timestamp clears active line");
    assert.equal(jobs.length, 2);
  } finally { ui.dispose(); }
});

test("automatic local read waits until the exact track identity is available", async () => {
  const calls = [];
  const ui = harness({ async getCurrentLocalLyrics(identity) {
    calls.push(identity);
    return { kind: "found", content: "[00:01.00]Synthetic first\n[00:02.00]Synthetic second" };
  } });
  try {
    let tree = ui.render({ track: null });
    assert.equal(calls.length, 0, "no read is requested before playback exposes an identity");
    const current = { current_media: media("sqlite-track://fixture/track"), elapsed: 1.25, state: "play" };
    tree = ui.render({ track: current });
    assert.equal(calls.length, 1, "the newly available identity starts the automatic read");
    await flush();
    tree = ui.render({ track: current });
    assert.match(text(tree), /Synthetic first/);
    assert.deepEqual(calls[0], current.current_media);
  } finally { ui.dispose(); }
});

test("missing, plain, unsupported and failed local reads remain distinct and clear previous track text", async () => {
  const replies = [
    { kind: "found", content: "A plain synthetic lyric\nsecond line" },
    { kind: "not_found" },
    { kind: "read_failed", reason: "permission_denied" },
    { kind: "read_failed", reason: "stale_track" },
    { kind: "invalid_content", reason: "invalid_utf8" },
  ];
  const calls = [];
  const ui = harness({ async getCurrentLocalLyrics(identity) { calls.push(identity); return replies.shift(); } });
  try {
    const renderLocal = async (uri, elapsed = 0) => { let tree = ui.render({ track: { current_media: media(uri), elapsed } }); await flush(); return ui.render({ track: { current_media: media(uri), elapsed } }); };
    let tree = await renderLocal("local-a.flac");
    assert.match(text(tree), /A plain synthetic lyric/); assert.equal(nodes(tree, n => n.props?.className === "np-lyrics-synchronized").length, 0);
    tree = await renderLocal("local-b.flac"); assert.match(text(tree), /notFound/); assert.doesNotMatch(text(tree), /A plain synthetic lyric/);
    tree = await renderLocal("local-c.flac"); assert.match(text(tree), /readError/);
    tree = await renderLocal("local-d.flac"); assert.match(text(tree), /stale/);
    tree = await renderLocal("local-e.flac"); assert.match(text(tree), /invalid/);
    const before = calls.length;
    const plexTrack = { current_media: { kind: "plex", server_id: "synthetic", part_key: "opaque", queue_index: 0 }, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 5 };
    tree = ui.render({ track: plexTrack }); tree = ui.render({ track: plexTrack });
    assert.doesNotMatch(text(tree), /private read detail/); assert.equal(calls.length, before, "Plex never reaches the local reader");
    assert.ok(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length, "identified Plex media can search online");
    const cueTrack = { current_media: media("sqlite-cue://source/segment"), title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 5 };
    tree = ui.render({ track: cueTrack }); tree = ui.render({ track: cueTrack });
    assert.equal(calls.length, before, "SQLite CUE is never passed to the local sidecar reader");
    assert.ok(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length, "CUE with identity can search online by track metadata");
  } finally { ui.dispose(); }
});

test("a failed read offers an explicit retry, while playback line selection follows snapshots rather than a clock", async () => {
  const calls = []; let fail = true;
  const ui = harness({ async getCurrentLocalLyrics(identity) { calls.push(identity); if (fail) throw new Error("private read detail"); return { kind: "found", content: "[00:01.00]first\n[00:02.00]second" }; } });
  try {
    const track = { current_media: media("retry-local.flac"), elapsed: 1.5, state: "play" };
    let tree = ui.render({ track }); await flush(); tree = ui.render({ track });
    assert.match(text(tree), /readError/); assert.doesNotMatch(text(tree), /private read detail/);
    nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.retry")[0].props.onClick();
    fail = false; tree = ui.render({ track }); await flush(); tree = ui.render({ track });
    assert.equal(calls.length, 2);
    assert.equal(nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line")).filter(line => line.props["aria-current"]).length, 1);
  } finally { ui.dispose(); }
});

test("leaving the Lyrics layout fences a read that completes after unmount", async () => {
  const job = deferred();
  const ui = harness({ getCurrentLocalLyrics: () => job.promise });
  ui.render({ track: { current_media: media("unmount-local.flac"), elapsed: 0 } });
  ui.dispose();
  job.resolve({ kind: "found", content: "[00:01.00]late" }); await flush();
  assert.equal(ui.lateWrites(), 0, "the unmounted presentation receives no asynchronous state update");
});

const onlineRecord = (id, album, lyrics, extra = {}) => ({ id, trackName: "Synthetic track", artistName: "Synthetic artist", albumName: album,
  duration: 120, instrumental: false, hasWordSync: false, plainLyrics: null, syncedLyrics: lyrics, ...extra });
const track = (uri = "local-online.flac", duration = 120) => ({ current_media: media(uri), title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration, elapsed: 1.2, state: "play" });

test("local lyrics stay first; manual alternative can select another recording and return to local", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const candidates = [onlineRecord(11, "Studio edition", "[00:01.00]Online studio"), onlineRecord(12, "Live at Hall", "[00:01.00]Online live")];
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "found", content: "[00:01.00]Local version" }) }, async q => { calls.push(q); return { kind: "records", records: candidates }; });
  try {
    let tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() });
    assert.match(text(tree), /Local version/); assert.equal(calls.length, 0, "a valid local LRC prevents automatic lookup");
    const search = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0];
    await search.props.onClick(); await flush(); tree = ui.render({ track: track() });
    assert.equal(calls.length, 1); assert.match(text(tree), /Local version/, "searching candidates does not replace the local choice");
    const live = nodes(tree, node => node.type === "button" && text(node).includes("Live at Hall"))[0];
    live.props.onClick(); tree = ui.render({ track: track() }); assert.match(text(tree), /Online live/); assert.doesNotMatch(text(tree), /Local version/);
    nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.returnLocal")[0].props.onClick();
    tree = ui.render({ track: track() }); assert.match(text(tree), /Local version/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("automatic lookup respects enabled preference and rejects clear recording-version mismatches", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const fixtures = [onlineRecord(21, "Studio edition", "[00:01.00]Exact studio"),
    onlineRecord(22, "Live at Hall", "[00:01.00]Different live", { trackName: "Synthetic track (Live)" })];
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async q => { calls.push(q); return { kind: "records", records: fixtures }; });
  try {
    let tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() });
    assert.equal(calls.length, 1); assert.match(text(tree), /Exact studio/); assert.doesNotMatch(text(tree), /Different live/);
    const { cacheKey, ...sentMetadata } = calls[0];
    assert.deepEqual(sentMetadata, { title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120 });
    assert.equal(cacheKey, lyricsKey(sentMetadata));
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("automatic lookup defaults on only when no choice was saved and can adopt one clear editorial edition match", async () => {
  const previousWindow = globalThis.window, calls = [];
  const saved = new Map();
  globalThis.window = { localStorage: { getItem: key => saved.get(key) ?? null, setItem: (key, value) => saved.set(key, value) }, matchMedia: () => ({ matches: true }), dispatchEvent() {} };
  const candidate = onlineRecord(26, "Studio edition (Super Deluxe Edition) (Remastered)", "[00:01.00]Best edition", { duration: 120.7 });
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async query => { calls.push(query); return { kind: "records", records: [candidate] }; });
  try {
    let tree = ui.render({ track: track("default-auto.flac") }); await flush(); tree = ui.render({ track: track("default-auto.flac") }); await flush(); tree = ui.render({ track: track("default-auto.flac") });
    assert.equal(readLyricsOnlinePreference(), true);
    assert.equal(calls.length, 1, "an unset preference enables the requested default lookup");
    assert.match(text(tree), /Best edition/);
    assert.doesNotMatch(text(tree), /chooseVersion|onlinePrivacy|autoLookup|notFound/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("automatic search identity includes updated recording metadata on the same media locator", async () => {
  const previousWindow = globalThis.window, pending = [], signals = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }),
    onLyricsLookup(signal) { signals.push(signal); } }, async query => {
      const job = deferred(); pending.push({ query, ...job }); return job.promise;
    });
  try {
    const studio = track("same-media.flac");
    let tree = ui.render({ track: studio }); await flush(); tree = ui.render({ track: studio }); await flush();
    assert.equal(pending.length, 1);

    const live = { ...studio, title: "Synthetic track (Live)" };
    tree = ui.render({ track: live }); await flush();
    assert.equal(signals[0].aborted, true, "metadata change cancels the old query consumer");
    assert.equal(pending.length, 2, "the revised recording metadata gets its own search");
    assert.equal(pending[1].query.title, "Synthetic track (Live)");

    pending[0].resolve({ kind: "records", records: [onlineRecord(61, "Studio edition", "[00:01.00]Stale studio")] });
    pending[1].resolve({ kind: "records", records: [onlineRecord(62, "Studio edition", "[00:01.00]Current live", { trackName: "Synthetic track (Live)" })] });
    await flush(); tree = ui.render({ track: live });
    assert.match(text(tree), /Current live/);
    assert.doesNotMatch(text(tree), /Stale studio/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("Plex and CUE search online from musical metadata only; CUE elapsed is used as the segment clock", async () => {
  const previousWindow = globalThis.window, calls = [], localReads = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const cueLyrics = onlineRecord(71, "Cue album", "[00:02.00]CUE relative second\n[00:04.00]CUE relative fourth", { trackName: "Cue track", artistName: "Cue artist", duration: 12 });
  const ui = harness({ async getCurrentLocalLyrics(identity) { localReads.push(identity); return { kind: "not_eligible" }; } }, async query => { calls.push(query); return { kind: "records", records: [cueLyrics] }; });
  try {
    const cue = { current_media: media("sqlite-cue://private-source/opaque-segment"), title: "Cue track", artist: "Cue artist", album: "Cue album", duration: 12, elapsed: 2.4, state: "play" };
    let tree = ui.render({ track: cue }); await flush(); tree = ui.render({ track: cue }); await flush(); tree = ui.render({ track: cue });
    assert.match(text(tree), /CUE relative second/);
    const active = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.equal(active[0].props["aria-current"], "true", "elapsed is used directly as the current CUE queue item's timeline");
    assert.equal(localReads.length, 0, "CUE locator never goes to local filesystem reader");
    assert.deepEqual(Object.fromEntries(Object.entries(calls[0]).filter(([key]) => key !== "cacheKey")), { title: "Cue track", artist: "Cue artist", album: "Cue album", duration: 12 });
    assert.equal(calls[0].cacheKey, lyricsKey(calls[0]));
    assert.doesNotMatch(JSON.stringify(calls[0]), /sqlite-cue|private-source|segment/);

    const plex = { current_media: { kind: "plex", server_id: "secret-server", part_key: "secret-part", queue_index: 2 }, title: "Cue track", artist: "Cue artist", album: "Cue album", duration: 12, elapsed: 2.5, state: "play" };
    tree = ui.render({ track: plex }); await flush(); tree = ui.render({ track: plex }); await flush(); tree = ui.render({ track: plex });
    assert.equal(calls.length, 2, "a new identified Plex item gets its own automatic lookup");
    assert.deepEqual(Object.fromEntries(Object.entries(calls[1]).filter(([key]) => key !== "cacheKey")), { title: "Cue track", artist: "Cue artist", album: "Cue album", duration: 12 });
    assert.equal(localReads.length, 0);
  } finally { ui.dispose(); globalThis.window = previousWindow; }

  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const optedOutCalls = [];
  const optedOut = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_eligible" }) }, async query => { optedOutCalls.push(query); return { kind: "not_found" }; });
  try {
    const cue = { current_media: media("sqlite-cue://private/segment"), title: "Cue track", artist: "Cue artist", album: "Cue album", duration: 12, elapsed: 1 };
    optedOut.render({ track: cue }); await flush(); optedOut.render({ track: cue });
    assert.equal(optedOutCalls.length, 0, "explicit opt-out also suppresses automatic CUE lookup");
    const visible = optedOut.render({ track: cue });
    assert.ok(nodes(visible, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length, "manual online search remains available after opt-out");
  } finally { optedOut.dispose(); globalThis.window = previousWindow; }
});

test("queue-uncertain or incomplete media identity never starts an online request", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async query => { calls.push(query); return { kind: "not_found" }; });
  try {
    const identified = { current_media: media("sqlite-track://uncertain"), title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120 };
    let tree = ui.render({ track: { ...identified, snapshot_consistent: false } }); await flush(); tree = ui.render({ track: { ...identified, snapshot_consistent: false } });
    assert.equal(calls.length, 0, "an uncertain queue snapshot is not treated as an eligible permanent source");
    assert.match(text(tree), /identityPending/, "uncertain identity is not mislabeled as an unsupported source");
    assert.equal(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length, 0);
    tree = ui.render({ track: { current_media: { kind: "local", uri: "sqlite-track://incomplete" }, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120 } });
    await flush(); tree = ui.render({ track: { current_media: { kind: "local", uri: "sqlite-track://incomplete" }, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120 } });
    assert.equal(calls.length, 0, "media without a valid queue identity cannot trigger online lookup");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("first-track search resumes after a transient uncertain snapshot cancels the in-flight request", async () => {
  const previousWindow = globalThis.window, jobs = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }), onLyricsLookup(signal) {
    const job = { ...deferred(), signal }; jobs.push(job);
  } }, async () => jobs.at(-1).promise);
  try {
    const first = { current_media: media("sqlite-track://album-first-track"), title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 0, state: "play", snapshot_consistent: true };
    ui.render({ track: first }); await flush();
    let tree = ui.render({ track: first }); await flush();
    tree = ui.render({ track: first });
    assert.equal(jobs.length, 1, "confirmed first-track identity automatically begins lookup after local absence");
    assert.match(text(tree), /searching/, "a started automatic lookup is visible as searching");
    assert.doesNotMatch(text(tree), /nowPlaying\.lyrics\.notFound/, "local absence is not shown as a final search result");

    const temporarilyUncertain = { ...first, snapshot_consistent: false, current_media: null };
    ui.render({ track: temporarilyUncertain });
    assert.equal(jobs[0].signal.aborted, true, "the previous identity's result is fenced when the snapshot becomes uncertain");

    ui.render({ track: first }); await flush(); ui.render({ track: first });
    assert.equal(jobs.length, 2, "the same first track can restart while cancellation of its old request is still settling");
    jobs[0].reject(new DOMException("Canceled", "AbortError")); await flush();
    ui.render({ track: first });
    assert.equal(jobs.length, 2, "the older cancellation cannot clear or duplicate the replacement attempt");
    jobs[1].resolve({ kind: "records", records: [onlineRecord(81, "Studio edition", "[00:01.00]First album track lyric")] });
    await flush();
    const treeAfterSearch = ui.render({ track: first });
    assert.match(text(treeAfterSearch), /First album track lyric/, "eligible result appears without a click or next-track transition");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("first album track waits for confirmed identity, late metadata and the loaded preference before auto-search", async () => {
  const previousWindow = globalThis.window, calls = [], listeners = new Map();
  let preference = "false";
  globalThis.window = {
    localStorage: { getItem: () => preference, setItem() {} },
    matchMedia: () => ({ matches: true }),
    addEventListener: (name, listener) => listeners.set(name, listener),
    removeEventListener: name => listeners.delete(name),
  };
  const ui = harness({ async getCurrentLocalLyrics() { calls.push("local"); return { kind: "not_found" }; } }, async query => {
    calls.push(query);
    return { kind: "records", records: [onlineRecord(91, "Studio edition", "[00:01.00]First track found automatically")] };
  });
  try {
    let tree = ui.render({ track: null });
    assert.equal(calls.length, 0, "opening Lyrics before playback has no stale identity to search");

    const uncertain = { current_media: null, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 0, state: "play", snapshot_consistent: false };
    tree = ui.render({ track: uncertain });
    assert.equal(calls.length, 0, "an uncertain first queue snapshot does not start local or online work");

    const identifiedButIncomplete = { ...uncertain, current_media: media("sqlite-track://first-album-track"), snapshot_consistent: true, artist: "", duration: 0 };
    tree = ui.render({ track: identifiedButIncomplete });
    await flush();
    tree = ui.render({ track: identifiedButIncomplete });
    assert.deepEqual(calls, ["local"], "confirmed identity permits local lookup, but incomplete metadata cannot search online");
    assert.match(text(tree), /metadataNeeded|notFound/);

    const complete = { ...identifiedButIncomplete, artist: "Synthetic artist", duration: 120 };
    tree = ui.render({ track: complete });
    preference = "true";
    listeners.get("lyrics-pref")?.();
    tree = ui.render({ track: complete });
    await flush();
    tree = ui.render({ track: complete });
    assert.equal(calls.filter(call => typeof call === "object").length, 1, "the late preference and duration jointly allow one query");
    assert.match(text(tree), /First track found automatically/, "the first track is adopted without a click or track switch");
    tree = ui.render({ track: { ...complete, elapsed: 3.5 } });
    assert.equal(calls.filter(call => typeof call === "object").length, 1, "elapsed snapshots do not trigger another search");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("a stale local identity is retried once after the same first-track snapshot becomes confirmed", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ async getCurrentLocalLyrics(identity) {
    calls.push(identity);
    return calls.length === 1 ? { kind: "read_failed", reason: "stale_track" } : { kind: "not_found" };
  } }, async () => ({ kind: "records", records: [onlineRecord(92, "Studio edition", "[00:01.00]Recovered first-track lyric")] }));
  try {
    const confirmed = { current_media: media("sqlite-track://same-first-track"), title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 0, state: "play", snapshot_consistent: true };
    ui.render({ track: confirmed }); await flush();
    let tree = ui.render({ track: confirmed });
    assert.match(text(tree), /stale/);

    const uncertain = { ...confirmed, snapshot_consistent: false, playback_error: { code: "queue_uncertain" } };
    tree = ui.render({ track: uncertain });
    assert.doesNotMatch(text(tree), /Recovered first-track lyric/);
    tree = ui.render({ track: confirmed }); await flush();
    tree = ui.render({ track: confirmed }); await flush();
    tree = ui.render({ track: confirmed });
    assert.equal(calls.length, 2, "the same identity is re-read once after queue confirmation, without polling");
    assert.match(text(tree), /Recovered first-track lyric/, "local absence then allows automatic online lookup");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("a stale local lookup resumes once when usable recording metadata arrives for the same identity", async () => {
  const previousWindow = globalThis.window, identities = [], searches = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ async getCurrentLocalLyrics(identity) {
    identities.push(identity);
    return identities.length === 1 ? { kind: "read_failed", reason: "stale_track" } : { kind: "not_found" };
  } }, async query => { searches.push(query); return { kind: "records", records: [onlineRecord(93, "Studio edition", "[00:01.00]Metadata-recovered lyric")] }; });
  try {
    const identity = media("sqlite-track://metadata-late-first-track");
    const incomplete = { current_media: identity, title: "", artist: "", album: "", duration: 0, elapsed: 0, state: "play", snapshot_consistent: true };
    ui.render({ track: incomplete }); await flush();
    let tree = ui.render({ track: incomplete });
    assert.match(text(tree), /stale/);
    assert.equal(searches.length, 0, "incomplete metadata is not searched");

    const complete = { ...incomplete, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120 };
    tree = ui.render({ track: complete }); await flush();
    tree = ui.render({ track: complete }); await flush();
    tree = ui.render({ track: complete });
    assert.equal(identities.length, 2, "metadata advancement causes one revalidation of the stale local identity");
    assert.equal(searches.length, 1);
    assert.doesNotMatch(text(tree), /nowPlaying\.lyrics\.stale/, "the stale local result is cleared once the identity is revalidated");
    tree = ui.render({ track: { ...complete, elapsed: 2 } });
    assert.equal(identities.length, 2, "elapsed changes do not repeat the local read");
    assert.equal(searches.length, 1, "elapsed changes do not repeat the online search");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("automatic lookup waits for valid identity metadata and starts once it becomes available", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async query => { calls.push(query); return { kind: "records", records: [onlineRecord(72, "Studio edition", "[00:01.00]Late metadata lyric")] }; });
  try {
    const identity = media("sqlite-track://same-item");
    let tree = ui.render({ track: { current_media: identity, elapsed: 1 } }); await flush(); tree = ui.render({ track: { current_media: identity, elapsed: 1 } });
    assert.equal(calls.length, 0);
    const ready = { current_media: identity, title: "Synthetic track", artist: "Synthetic artist", album: "Studio edition", duration: 120, elapsed: 1 };
    tree = ui.render({ track: ready }); await flush(); tree = ui.render({ track: ready });
    assert.equal(calls.length, 1, "metadata becoming available changes the query key and starts one automatic lookup");
    assert.match(text(tree), /Late metadata lyric/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("missing local lyrics do not query automatically while opt-out; manual lookup remains available after read errors", async () => {
  const previousWindow = globalThis.window, calls = [];
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async query => { calls.push(query); return { kind: "records", records: [onlineRecord(25, "Studio edition", "[00:01.00]Manual only")] }; });
  try {
    let tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() });
    assert.equal(calls.length, 0, "opt-out prevents automatic search");
    assert.ok(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length);
    const button = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline")[0];
    await button.props.onClick(); await flush(); tree = ui.render({ track: track() });
    assert.equal(calls.length, 1); assert.doesNotMatch(text(tree), /Manual only/, "manual results are not adopted without explicit choice");
    nodes(tree, node => node.type === "button" && text(node).includes("Studio edition"))[0].props.onClick();
    tree = ui.render({ track: track() }); assert.match(text(tree), /Manual only/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }

  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const failedRead = harness({ getCurrentLocalLyrics: async () => ({ kind: "read_failed", reason: "permission_denied" }) });
  try {
    let tree = failedRead.render({ track: track("permission-test.flac") }); await flush(); tree = failedRead.render({ track: track("permission-test.flac") });
    assert.match(text(tree), /readError/);
    assert.ok(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline").length, "local read failure is still visible and offers manual search");
  } finally { failedRead.dispose(); globalThis.window = previousWindow; }
});

test("multiple eligible results adopt the deterministic best immediately; unmatched and durationless sync remain alternatives", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ambiguous = [onlineRecord(31, "Studio edition", "[00:01.00]First"), onlineRecord(32, "Studio edition", "[00:01.00]Second")];
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async () => ({ kind: "records", records: ambiguous }));
  try {
    let tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() });
    assert.match(text(tree), /First/, "first eligible ranked result is adopted without asking because alternatives exist");
    assert.equal(nodes(tree, node => node.type === "button" && node.props['aria-pressed'] === false).length, 0, "candidate list stays collapsed after automatic adoption");
    const other = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0];
    assert.ok(other, "alternatives remain available through the secondary action");
  } finally { ui.dispose(); globalThis.window = previousWindow; }

  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const noDuration = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async () => ({ kind: "records", records: [onlineRecord(33, "Studio edition", "[00:01.00]Need confirmation")] }));
  try {
    let tree = noDuration.render({ track: track("duration-unknown.flac", 0) }); await flush(); tree = noDuration.render({ track: track("duration-unknown.flac", 0) }); await flush(); tree = noDuration.render({ track: track("duration-unknown.flac", 0) });
    assert.equal(nodes(tree, node => node.type === "p" && node.props.className === "np-lyrics-line").length, 0);
    assert.match(text(tree), /onlineNoEligible/);
    assert.equal(nodes(tree, node => node.type === "button" && node.props['aria-pressed'] === false).length, 1);
  } finally { noDuration.dispose(); globalThis.window = previousWindow; }
});

test("explicit candidate selection keeps instrumental, plain and synchronized records distinct", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const candidates = [onlineRecord(51, "Score", null, { instrumental: true, plainLyrics: null }),
    onlineRecord(52, "Plain edition", null, { plainLyrics: "Synthetic unsynchronized words" })];
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async () => ({ kind: "records", records: candidates }));
  try {
    let tree = ui.render({ track: track("format-test.flac") }); await flush(); tree = ui.render({ track: track("format-test.flac") });
    const search = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline")[0];
    await search.props.onClick(); await flush(); tree = ui.render({ track: track("format-test.flac") });
    nodes(tree, node => node.type === "button" && text(node).includes("Score"))[0].props.onClick();
    tree = ui.render({ track: track("format-test.flac") }); assert.match(text(tree), /nowPlaying.lyrics.instrumental/); assert.doesNotMatch(text(tree), /nowPlaying.lyrics.onlineNotFound/);
    const searchAgain = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0];
    await searchAgain.props.onClick(); await flush(); tree = ui.render({ track: track("format-test.flac") });
    nodes(tree, node => node.type === "button" && text(node).includes("Plain edition"))[0].props.onClick();
    tree = ui.render({ track: track("format-test.flac") }); assert.match(text(tree), /Synthetic unsynchronized words/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("valid syncedLyrics wins over plainLyrics and follow scrolls only its container until resumed", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: false }) };
  const record = onlineRecord(40, "Studio edition", "[00:01.00]Synced first\n[00:02.00]Synced second", {
    plainLyrics: "Plain fallback that must not be shown", hasWordSync: true,
  });
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async () => ({ kind: "records", records: [record] }));
  try {
    const atFirst = track("sync-online.flac"); atFirst.elapsed = 1.5;
    let tree = ui.render({ track: atFirst }); await flush(); tree = ui.render({ track: atFirst });
    const search = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline")[0];
    await search.props.onClick(); await flush(); tree = ui.render({ track: atFirst });
    nodes(tree, node => node.type === "button" && text(node).includes("Studio edition"))[0].props.onClick();
    tree = ui.render({ track: atFirst });
    let lines = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.deepEqual(lines.map(text), ["Synced first", "Synced second"]);
    assert.equal(lines[0].props["aria-current"], "true");
    assert.doesNotMatch(text(tree), /Plain fallback|chooseVersion|onlinePrivacy|autoLookup/);
    assert.equal(ui.scrolls(), 1);

    const container = nodes(tree, node => node.type === "div" && node.props.className?.includes("np-lyrics-synchronized"))[0];
    container.props.onWheel();
    const later = { ...atFirst, elapsed: 2.5 };
    tree = ui.render({ track: later });
    lines = nodes(tree, node => node.type === "p" && node.props.className?.includes("np-lyrics-line"));
    assert.equal(lines[1].props["aria-current"], "true", "seek/new elapsed snapshots still choose the right line");
    assert.equal(ui.scrolls(), 1, "manual browsing pauses automatic following");
    const resume = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.resumeFollow")[0];
    assert.ok(resume); resume.props.onClick(); tree = ui.render({ track: later });
    assert.equal(ui.scrolls(), 2, "resuming centers the current line in the lyrics container");
    assert.equal(ui.scrollRequests().at(-1).top, 50, "the active line is centered within the lyrics container");
    assert.equal(ui.scrollRequests().at(-1).behavior, "smooth");
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("online failure leaves current local lyrics visible and an obsolete lookup cannot replace the next track", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const lookup = deferred(); let lookupSignal;
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "found", content: "[00:01.00]Keep local" }),
    onLyricsLookup(signal) { lookupSignal = signal; } }, async () => lookup.promise);
  try {
    let tree = ui.render({ track: track() }); await flush(); tree = ui.render({ track: track() });
    const search = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0];
    const pending = search.props.onClick();
    await flush();
    assert.ok(lookupSignal, "the current request reached the mocked online provider");
    tree = ui.render({ track: track("next-local.flac") }); await flush();
    assert.equal(lookupSignal.aborted, true, "changing tracks cancels the obsolete lookup consumer");
    lookup.resolve({ kind: "records", records: [onlineRecord(41, "Studio edition", "[00:01.00]Stale online")] }); await pending;
    tree = ui.render({ track: track("next-local.flac") });
    assert.doesNotMatch(text(tree), /Stale online/); assert.match(text(tree), /Keep local/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("a manually selected version is restored from durable choice for A to B to A, independent of queue index and network", async () => {
  const previousWindow = globalThis.window, savedChoices = new Map(), writes = [], searches = [];
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const identityKey = item => item.kind === "local" ? JSON.stringify(["local", item.uri]) : JSON.stringify(["plex", item.server_id, item.part_key]);
  const service = {
    async getCurrentLocalLyrics() { return { kind: "not_found" }; },
    async getLyricsChoice(item) { return savedChoices.get(identityKey(item)) ?? null; },
    async setLyricsChoice(item, choice) { writes.push(choice); savedChoices.set(identityKey(item), choice); },
  };
  const selected = onlineRecord(101, "Synthetic Album", "[00:01.00]Persisted offline selection");
  const ui = harness(service, async (_query, _signal, refresh) => { searches.push(refresh); return { kind: "records", records: [selected] }; });
  try {
    const trackA = { ...track("stable-a.flac"), current_media: media("sqlite-track://stable-a", 1) };
    let tree = ui.render({ track: trackA }); await flush(); tree = ui.render({ track: trackA });
    await nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline")[0].props.onClick();
    await flush(); tree = ui.render({ track: trackA });
    nodes(tree, node => node.type === "button" && text(node).includes("Synthetic Album"))[0].props.onClick();
    await flush(); tree = ui.render({ track: trackA });
    assert.match(text(tree), /Persisted offline selection/);
    assert.equal(savedChoices.get(identityKey(trackA.current_media)).kind, "manual");
    assert.deepEqual(searches, [false], "the first explicit lookup can restore the durable result offline");
    tree = ui.render({ track: trackA });
    await nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0].props.onClick();
    await flush();
    assert.deepEqual(searches, [false, true], "a subsequent explicit alternative search refreshes while retaining the prior displayed lyric");

    const trackB = { ...track("stable-b.flac"), current_media: media("sqlite-track://stable-b", 2) };
    ui.render({ track: trackB }); await flush(); ui.render({ track: trackB });
    const returnedA = { ...trackA, current_media: media("sqlite-track://stable-a", 8) };
    tree = ui.render({ track: returnedA }); await flush(); tree = ui.render({ track: returnedA });
    assert.match(text(tree), /Persisted offline selection/, "A is restored immediately when revisited at another queue position");
    assert.equal(searches.length, 2, "restoring a manual selection does not require another network request");
    assert.ok(writes.some(choice => choice.kind === "manual" && choice.record.id === selected.id));
    assert.ok(nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.useAutomatic").length);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("returning from B to an automatically matched A re-reads the durable result instead of remaining blank", async () => {
  const previousWindow = globalThis.window, lookups = [];
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async query => {
    lookups.push(query.title);
    return { kind: "records", persisted: true, records: [onlineRecord(141, query.album, `[00:01.00]Durable ${query.title}`, { trackName: query.title })] };
  });
  try {
    const trackA = { ...track("auto-a.flac"), title: "Track A", album: "Album A", current_media: media("sqlite-track://auto-a", 1) };
    let tree = ui.render({ track: trackA }); await flush(); tree = ui.render({ track: trackA }); await flush(); tree = ui.render({ track: trackA });
    assert.match(text(tree), /Durable Track A/);
    const trackB = { ...track("auto-b.flac"), title: "Track B", album: "Album B", current_media: media("sqlite-track://auto-b", 2) };
    ui.render({ track: trackB }); await flush(); ui.render({ track: trackB }); await flush(); tree = ui.render({ track: trackB });
    assert.match(text(tree), /Durable Track B/);
    const revisitA = { ...trackA, current_media: media("sqlite-track://auto-a", 9) };
    ui.render({ track: revisitA }); await flush(); tree = ui.render({ track: revisitA }); await flush(); tree = ui.render({ track: revisitA });
    assert.match(text(tree), /Durable Track A/, "the previous autoAttempted flag cannot leave a revisited track blank");
    assert.deepEqual(lookups, ["Track A", "Track B", "Track A"]);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("explicit local preference and return to automatic policy survive through choice writes", async () => {
  const previousWindow = globalThis.window, choices = [], calls = [];
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "found", content: "Local synthetic words" }),
    async setLyricsChoice(_media, choice) { choices.push(choice); } }, async () => { calls.push(true); return { kind: "records", records: [onlineRecord(111, "Synthetic Album", "[00:01.00]Remote alternative")] }; });
  try {
    const current = track("local-choice.flac");
    let tree = ui.render({ track: current }); await flush(); tree = ui.render({ track: current });
    const search = nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0];
    await search.props.onClick(); await flush(); tree = ui.render({ track: current });
    nodes(tree, node => node.type === "button" && text(node).includes("Synthetic Album"))[0].props.onClick(); await flush();
    tree = ui.render({ track: current });
    nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.returnLocal")[0].props.onClick(); await flush();
    tree = ui.render({ track: current });
    assert.match(text(tree), /Local synthetic words/);
    assert.deepEqual(choices.at(-1), { kind: "prefer_local" });
    nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.useAutomatic")[0].props.onClick(); await flush();
    assert.deepEqual(choices.at(-1), { kind: "automatic" }, "the user can clear the per-track local preference");
    assert.equal(calls.length, 1);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("a failed durable choice write keeps the selected lyric available in the current session", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => "false", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const selected = onlineRecord(121, "Synthetic Album", "[00:01.00]Session copy survives disk failure");
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }),
    async setLyricsChoice() { throw new Error("disk unavailable"); } }, async () => ({ kind: "records", records: [selected] }));
  try {
    const current = track("choice-write-failure.flac");
    let tree = ui.render({ track: current }); await flush(); tree = ui.render({ track: current });
    await nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOnline")[0].props.onClick();
    await flush(); tree = ui.render({ track: current });
    nodes(tree, node => node.type === "button" && text(node).includes("Synthetic Album"))[0].props.onClick();
    await flush(); tree = ui.render({ track: current });
    assert.match(text(tree), /Session copy survives disk failure/);
    assert.match(text(tree), /nowPlaying\.lyrics\.saveFailed/);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});

test("refreshing alternatives preserves the currently displayed valid result when the new response omits it", async () => {
  const previousWindow = globalThis.window; let calls = 0;
  globalThis.window = { localStorage: { getItem: () => "true", setItem() {} }, matchMedia: () => ({ matches: true }) };
  const original = onlineRecord(131, "Synthetic Album", "[00:01.00]Current saved lyric");
  const replacement = onlineRecord(132, "Synthetic Album", "[00:01.00]New alternative", { plainLyrics: null });
  const ui = harness({ getCurrentLocalLyrics: async () => ({ kind: "not_found" }) }, async () => ({ kind: "records", records: [calls++ === 0 ? original : replacement] }));
  try {
    const current = track("refresh-preserve.flac");
    let tree = ui.render({ track: current }); await flush(); tree = ui.render({ track: current }); await flush(); tree = ui.render({ track: current });
    assert.match(text(tree), /Current saved lyric/);
    await nodes(tree, node => node.type === "button" && node.props.children === "nowPlaying.lyrics.searchOther")[0].props.onClick();
    await flush(); tree = ui.render({ track: current });
    assert.match(text(tree), /Current saved lyric/, "the previous selected lyric remains the visible fallback during a successful refresh");
    assert.equal(calls, 2);
  } finally { ui.dispose(); globalThis.window = previousWindow; }
});
