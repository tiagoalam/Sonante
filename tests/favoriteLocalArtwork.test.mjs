import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import React from "react";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";
import { sqliteFavoriteArtworkIdentity, localAlbumArtworkIdentity } from "../src/utils/favoriteArtwork.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const equalDeps = (left, right) => left && right && left.length === right.length && left.every((value, index) => Object.is(value, right[index]));
let currentHost;
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
    if (!equalDeps(slot.deps, deps)) host.effects.push(() => {
      slot.cleanup?.(); slot.deps = deps; slot.cleanup = effect();
    });
  },
};

function compileFavoriteArtwork(artwork) {
  const exports = {};
  const reactModule = { __esModule: true, ...hooks, default: hooks };
  const dependencies = {
    react: reactModule,
    "react/jsx-runtime": jsx,
    "../utils/localArtworkSession": artwork,
    "../utils/frontendDiagnostics": { logError: (...args) => artwork.errors.push(args) },
  };
  const source = readFileSync(path.join(root, "src/components/FavoriteLocalArtwork.tsx"), "utf8");
  const compiled = ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  new Function("require", "exports", compiled)(name => {
    if (name in dependencies) return dependencies[name];
    throw new Error(`Unexpected favorite artwork dependency: ${name}`);
  }, exports);
  return exports.FavoriteLocalArtwork;
}

class HookHost {
  slots = [];
  cursor = 0;
  effects = [];
  render(component, props) {
    this.cursor = 0; this.effects = []; currentHost = this;
    const tree = component(props); currentHost = undefined;
    this.effects.forEach(effect => effect());
    return tree;
  }
  unmount() { this.slots.forEach(slot => slot.cleanup?.()); }
}

const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(node => nodes(node, predicate))
  : !tree || typeof tree !== "object" ? [] : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const album = (id = "sqlite:7:YWxidW0") => ({ id, title: "Fixture Album", artist: "Fixture Artist",
  folder_path: `sqlite-album://${id}`, track_count: 1, source_id: "sqlite-7", discs: [] });
const favorite = (item = album()) => ({ id: item.id, source: "local", title: item.title, artist: item.artist,
  path_or_key: item.folder_path, thumb: null, plex_image: null, exists: true });

function artworkFixture(readCover, restored = new Map()) {
  const thumbnail = "data:image/jpeg;base64," + Buffer.from([255, 216, 255, 192, 0, 11, 8, 0, 1, 0, 1, 1, 1, 17, 0, 255, 217]).toString("base64");
  const fixture = { cache: new Map(), listeners: new Map(), reads: [], captures: [], errors: [], restored, thumbnail };
  fixture.localArtworkResolver = {
    get(id) { return fixture.cache.get(id) ?? null; },
    subscribe(id, callback) {
      const listeners = fixture.listeners.get(id) ?? new Set(); listeners.add(callback); fixture.listeners.set(id, listeners);
      return () => listeners.delete(callback);
    },
    seedPersistedLocal(id, cover) { fixture.cache.set(id, cover); fixture.listeners.get(id)?.forEach(callback => callback(cover)); },
    async resolve(value, online, blocked, _phase, interested) {
      fixture.reads.push({ id: value.id, path: value.folder_path, online });
      const cover = await readCover(value);
      if (blocked() || !interested()) return fixture.cache.get(value.id) ?? null;
      if (cover) {
        fixture.cache.set(value.id, cover);
        fixture.listeners.get(value.id)?.forEach(callback => callback(cover));
      }
      return cover;
    },
  };
  fixture.localAlbumThumbnails = {
    get(value) { return fixture.restored.get(value.id) ?? null; },
    capture(value, image, current, trigger) {
      fixture.captures.push({ id: value.id, trigger });
      if (current()) fixture.restored.set(value.id, fixture.thumbnail);
    },
  };
  return fixture;
}

function props(fixture, item = album(), fav = favorite(item)) {
  return { favorite: fav, album: item, localAlbums: [item], artworkReady: true, alt: fav.title,
    className: "cover", fallback: React.createElement("span", null, "No artwork") };
}

test("SQLite favorite artwork matches the canonical album reference and never guesses from a shared ID", () => {
  const item = album();
  const local = favorite(item);
  assert.equal(sqliteFavoriteArtworkIdentity(local, true), JSON.stringify([item.id, item.folder_path]));
  assert.equal(localAlbumArtworkIdentity(item), JSON.stringify([item.id, item.folder_path]));
  assert.equal(sqliteFavoriteArtworkIdentity({ ...local, source: "plex" }, true), null);
  assert.equal(sqliteFavoriteArtworkIdentity({ ...local, path_or_key: "Music/Fixture Album" }, true), null);
  assert.equal(sqliteFavoriteArtworkIdentity(local, false), null);
  assert.notEqual(sqliteFavoriteArtworkIdentity({ ...local, path_or_key: "sqlite-album://sqlite:7:other" }, true),
    localAlbumArtworkIdentity(item), "the same opaque ID cannot override a different canonical path");
});

test("visible SQLite favorites resolve through the shared artwork resolver and persist the displayed cover", async () => {
  const item = album(), fav = favorite(item), fullCover = "data:image/png;base64,fixture-full";
  const fixture = artworkFixture(async value => { assert.equal(value, item); return fullCover; });
  const component = compileFavoriteArtwork(fixture), host = new HookHost(), viewProps = props(fixture, item, fav);
  host.render(component, viewProps); host.render(component, viewProps); await flush();
  let tree = host.render(component, viewProps);
  assert.deepEqual(fixture.reads, [{ id: item.id, path: item.folder_path, online: false }]);
  assert.equal(nodes(tree, node => node.type === "img")[0]?.props.src, fullCover);
  const image = nodes(tree, node => node.type === "img")[0];
  image.props.onLoad({ currentTarget: { currentSrc: fullCover, src: fullCover } });
  assert.equal(fixture.restored.get(item.id), fixture.thumbnail);
  assert.equal(fixture.captures[0]?.id, item.id);
  host.unmount();

  const afterNavigation = new HookHost();
  tree = afterNavigation.render(component, viewProps);
  await flush(); tree = afterNavigation.render(component, viewProps);
  assert.equal(nodes(tree, node => node.type === "img")[0]?.props.src, fullCover);
  assert.equal(fixture.reads.length, 1, "navigation reuses the in-memory artwork cache");
  afterNavigation.unmount();

  fixture.cache.clear(); // simulate a fresh application session
  const afterRestart = new HookHost();
  tree = afterRestart.render(component, viewProps);
  await flush(); tree = afterRestart.render(component, viewProps);
  assert.equal(nodes(tree, node => node.type === "img")[0]?.props.src, fixture.thumbnail);
  assert.equal(fixture.reads.length, 1, "the persisted, generation-scoped thumbnail avoids another source read");
  assert.equal(fixture.cache.get(item.id), fixture.thumbnail, "the persisted thumbnail reseeds the shared resolver");
  afterRestart.unmount();
});

test("missing SQLite artwork keeps the fallback, while legacy Local artwork uses its saved thumbnail", async () => {
  const item = album(), fav = favorite(item);
  const fixture = artworkFixture(async () => null), component = compileFavoriteArtwork(fixture), host = new HookHost();
  const viewProps = props(fixture, item, fav);
  let tree = host.render(component, viewProps);
  host.render(component, viewProps); await flush();
  tree = host.render(component, viewProps);
  assert.equal(nodes(tree, node => node.type === "img").length, 0);
  assert.equal(nodes(tree, node => node.type === "span" && node.props.children === "No artwork").length, 1);
  assert.equal(fixture.reads.length, 1);
  host.unmount();

  const legacy = { ...fav, path_or_key: "Music/Fixture Album", thumb: "legacy-local-cover" };
  const legacyHost = new HookHost();
  tree = legacyHost.render(component, { ...props(fixture, null, legacy), album: null });
  assert.equal(nodes(tree, node => node.type === "img")[0]?.props.src, "legacy-local-cover");
  assert.equal(fixture.reads.length, 1, "legacy favorites do not query the SQLite artwork resolver");
  legacyHost.unmount();
});

test("FavoritesView passes exact SQLite identity and keeps Plex artwork on its existing path", () => {
  const view = readFileSync(path.join(root, "src/components/FavoritesView.tsx"), "utf8");
  const app = readFileSync(path.join(root, "src/App.tsx"), "utf8");
  assert.match(view, /sqliteFavoriteArtworkIdentity\(favorite, sqliteBacked\)/);
  assert.match(view, /FavoriteLocalArtwork favorite=\{fav\} album=\{sqliteAlbumForFavorite\(fav\)\}/);
  assert.match(view, /fav\.source === "plex" && fav\.plex_image \? \(/);
  assert.match(app, /localAlbums=\{sqliteMode \? localCatalog\.albums : undefined\}/);
  assert.match(app, /sqliteArtworkReady=\{sqliteMode && sqliteSourcesLoaded && localCatalog\.loaded/);
});
