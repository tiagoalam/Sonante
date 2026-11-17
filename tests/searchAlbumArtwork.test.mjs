import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import React from "react";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";
import * as icons from "lucide-react";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
let currentHost;
const sameDeps = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
const hooks = {
  ...React,
  useState(initial) {
    const host = currentHost, index = host.cursor++;
    const slot = host.slots[index] ?? (host.slots[index] = { value: typeof initial === "function" ? initial() : initial });
    return [slot.value, next => { slot.value = typeof next === "function" ? next(slot.value) : next; }];
  },
  useRef(initial) {
    const host = currentHost, index = host.cursor++;
    return host.slots[index] ?? (host.slots[index] = { current: initial });
  },
  useEffect(effect, deps) {
    const host = currentHost, index = host.cursor++;
    const slot = host.slots[index] ?? (host.slots[index] = {});
    if (!sameDeps(slot.deps, deps)) host.effects.push(() => { slot.cleanup?.(); slot.deps = deps; slot.cleanup = effect(); });
  },
};
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(value => nodes(value, predicate))
  : !tree || typeof tree !== "object" ? [] : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const flush = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };

function compile(artwork, IntersectionObserver = class {}) {
  const exports = {};
  const dependencies = {
    react: { __esModule: true, ...hooks, default: hooks }, "react/jsx-runtime": jsx, "lucide-react": icons,
    "./PlexImage": { PlexImage: function PlexImage() {} }, "./NavidromeView": { Artwork: function NavArtwork() {} },
    "../utils/localArtworkSession": artwork,
    "../utils/frontendDiagnostics": { logError: (...args) => artwork.errors.push(args) },
  };
  const source = readFileSync(path.join(root, "src/components/SearchAlbumArtwork.tsx"), "utf8");
  const compiled = ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  new Function("require", "exports", "IntersectionObserver", compiled)(name => {
    if (name in dependencies) return dependencies[name];
    throw new Error(`Unexpected search artwork dependency: ${name}`);
  }, exports, IntersectionObserver);
  return exports;
}

function artworkFixture({ cached = new Map(), thumbnails = new Map(), resolve = async () => null } = {}) {
  const fixture = { errors: [], calls: [], seeds: [], subscribers: new Map(), cached, thumbnails, resolve };
  fixture.localArtworkResolver = {
    get: id => fixture.cached.get(id) ?? null,
    subscribe(id, listener) {
      const listeners = fixture.subscribers.get(id) ?? new Set(); listeners.add(listener); fixture.subscribers.set(id, listeners);
      return () => listeners.delete(listener);
    },
    seedPersistedLocal(id, cover) {
      fixture.seeds.push([id, cover]); fixture.cached.set(id, cover);
      fixture.subscribers.get(id)?.forEach(listener => listener(cover));
    },
    resolve(album, online, blocked, _phase, interested) {
      fixture.calls.push({ album, online, blocked, interested });
      return fixture.resolve(album, online, blocked, interested);
    },
  };
  fixture.localAlbumThumbnails = { get: album => fixture.thumbnails.get(album.id) ?? null };
  return fixture;
}

class HookHost {
  slots = [];
  cursor = 0;
  effects = [];
  render(component, props) {
    this.cursor = 0; this.effects = []; currentHost = this;
    const tree = component(props); currentHost = undefined;
    nodes(tree, node => node.props?.ref && typeof node.props.ref === "object").forEach(node => { node.props.ref.current = {}; });
    this.effects.forEach(effect => effect());
    return tree;
  }
  unmount() { this.slots.forEach(slot => slot.cleanup?.()); }
}

test("album search artwork routes each source through its existing identity-aware cache", () => {
  const artwork = artworkFixture(), { SearchAlbumArtwork, LocalSearchAlbumArtwork } = compile(artwork);
  const plexIdentity = {};
  const shared = { active: true, localReady: true, sqliteBacked: true, onlineArtworkEnabled: true, isLibraryUpdating: false, plexIdentity };
  const localAlbum = { id: "sqlite:12:YWxidW0", title: "Fixture", artist: "Local", folder_path: "sqlite-album://sqlite:12:YWxidW0", discs: [] };
  const plexImage = { server_id: "plex-server", path: "/library/metadata/73/thumb/1" };
  const plex = SearchAlbumArtwork({ ...shared, row: { source: "plex", kind: "album", key: "plex:album:73", album: { rating_key: "73", title: "Fixture", artist: "Plex", thumb: plexImage } } });
  assert.equal(plex.props.className?.includes("h-11"), true);
  const plexNode = plex.props.children;
  assert.equal(plexNode.type.name, "PlexImage");
  assert.equal(plexNode.props.image, plexImage);
  assert.equal(plexNode.props.cacheScope, plexIdentity);
  assert.equal(plexNode.props.alt, "");

  const nav = SearchAlbumArtwork({ ...shared, row: { source: "navidrome", kind: "album", key: "navidrome:account:album:4",
    album: { id: "4", title: "Fixture", artist: "Nav", cover_id: "cover-4" }, connection: { connection_id: "connection-2", account_id: "account" } } });
  const navNode = nav.props.children;
  assert.equal(navNode.type.name, "NavArtwork");
  assert.deepEqual([navNode.props.connection, navNode.props.id], ["connection-2", "cover-4"]);

  const local = SearchAlbumArtwork({ ...shared, row: { source: "local", kind: "album", key: "local:album:sqlite:12:YWxidW0", album: localAlbum } });
  assert.equal(local.type, LocalSearchAlbumArtwork);
  assert.equal(local.props.album, localAlbum);
  assert.equal(local.props.sqliteBacked, true);
  assert.equal(local.props.onlineArtworkEnabled, true);
  assert.ok(SearchAlbumArtwork({ ...shared, row: { source: "plex", kind: "album", key: "plex:album:missing", album: { rating_key: "missing", title: "Missing", artist: "Plex" } } }).props.children);
});

test("local search artwork waits for visibility, honors source gates, and reuses saved thumbnails", async () => {
  const observers = [];
  class Observer {
    constructor(callback, options) { this.callback = callback; this.options = options; observers.push(this); }
    observe() {}
    disconnect() {}
    enter() { this.callback([{ isIntersecting: true }]); }
  }
  const cover = "data:image/jpeg;base64,fixture";
  const fixture = artworkFixture({ resolve: async (_album, _online, _blocked, interested) => interested() ? cover : null });
  const { LocalSearchAlbumArtwork } = compile(fixture, Observer);
  const album = { id: "sqlite:12:YWxidW0", title: "Fixture", artist: "Local", folder_path: "sqlite-album://sqlite:12:YWxidW0", discs: [] };
  const host = new HookHost(), props = { album, active: true, localReady: true, sqliteBacked: true, onlineArtworkEnabled: true, isLibraryUpdating: false };
  host.render(LocalSearchAlbumArtwork, props);
  assert.equal(fixture.calls.length, 0, "mounting an offscreen card performs no cover read");
  assert.equal(observers[0].options.rootMargin, "150px");
  observers[0].enter(); host.render(LocalSearchAlbumArtwork, props);
  assert.equal(fixture.calls.length, 1);
  assert.equal(fixture.calls[0].online, false, "SQLite artwork never starts online lookups");
  assert.equal(fixture.calls[0].interested(), true);
  await flush();
  let tree = host.render(LocalSearchAlbumArtwork, props);
  const image = nodes(tree, node => node.type === "img")[0];
  assert.equal(image?.props.src, cover);
  image.props.onError();
  tree = host.render(LocalSearchAlbumArtwork, props);
  assert.equal(fixture.calls.length, 1, "a broken image falls back without retrying the same source request");
  assert.equal(nodes(tree, node => node.type === "img").length, 0);
  host.unmount();

  const saved = artworkFixture({ thumbnails: new Map([[album.id, "persisted-small-preview"]]) });
  const savedComponent = compile(saved, Observer).LocalSearchAlbumArtwork;
  const savedHost = new HookHost();
  savedHost.render(savedComponent, props);
  assert.equal(saved.calls.length, 0, "a saved thumbnail paints without reading the source artwork");
  assert.deepEqual(saved.seeds, [[album.id, "persisted-small-preview"]]);
  observers.at(-1).enter();
  tree = savedHost.render(savedComponent, props);
  assert.equal(saved.calls.length, 0);
  assert.ok(nodes(tree, node => node.type === "img").length === 1);
  savedHost.unmount();
});

test("hidden search artwork does not observe or request covers", () => {
  const observers = [];
  class Observer { constructor(callback) { this.callback = callback; observers.push(this); } observe() {} disconnect() {} }
  const fixture = artworkFixture(), { LocalSearchAlbumArtwork } = compile(fixture, Observer), host = new HookHost();
  const album = { id: "local-hidden", title: "Hidden", artist: "Local", folder_path: "Music/Hidden", discs: [] };
  host.render(LocalSearchAlbumArtwork, { album, active: false, localReady: true, sqliteBacked: false, onlineArtworkEnabled: false, isLibraryUpdating: false });
  assert.equal(observers.length, 0);
  assert.equal(fixture.calls.length, 0);
  host.unmount();
});
