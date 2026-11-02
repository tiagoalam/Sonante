import { editionProfile, type EditionProfile } from './albumEditionAnalysis';
import type { EditionEntry } from './albumEditions';

// Descriptive observations only. This store cannot authorize a source read or
// playback. It is opened lazily by Editions, never by application startup.
const MAX_RECORDS = 16_000, MAX_BYTES = 32 * 1024 * 1024, MAX_RECORD_BYTES = 64 * 1024;
const MAX_AGE = 30 * 24 * 60 * 60 * 1000;
export interface EditionArchiveRecord { key: string; at: number; weight: number; profile: EditionProfile }
export interface EditionArchiveStorage {
  get(keys: readonly string[]): Promise<Map<string, unknown>>;
  put(record: EditionArchiveRecord): Promise<void>;
  clear(namespace: string, keys?: readonly string[]): Promise<void>;
}
const validKey = (key: unknown): key is string => typeof key === 'string' && /^[a-f0-9]{64}:[a-f0-9]{64}$/u.test(key);
function record(value: unknown, key: string, now: number): EditionArchiveRecord | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const raw = value as EditionArchiveRecord, profile = editionProfile(raw.profile);
  if (raw.key !== key || !validKey(key) || !Number.isSafeInteger(raw.at) || now < raw.at || now - raw.at >= MAX_AGE || !profile?.sizes) return null;
  const weight = JSON.stringify({ key, at: raw.at, profile }).length * 2;
  return weight <= MAX_RECORD_BYTES ? { key, at: raw.at, profile, weight } : null;
}
const stable = (value: unknown): unknown => Array.isArray(value) ? value.map(stable)
  : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([key, v]) => [key, stable(v)])) : value;
export function editionArchiveFingerprint(entry: EditionEntry, sourceContext: unknown): string {
  // The Navidrome connection ID changes on every launch; account/album IDs do
  // not. No connection IDs, read scopes or catalog counters go into durable keys.
  return JSON.stringify([entry.source === 'plex' ? 2 : 1, entry.key, stable(entry.album), stable(sourceContext)]);
}
export async function editionArchiveDigest(value: string): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}
export function createEditionArchive(storage: EditionArchiveStorage, digest = editionArchiveDigest, now = Date.now) {
  return {
    clear: (namespace: string, keys?: readonly string[]) => {
      if (!/^[a-f0-9]{64}$/u.test(namespace) || keys && (keys.length > MAX_RECORDS || keys.some(key => !validKey(key) || !key.startsWith(namespace + ':')))) return Promise.reject('EDITIONS_ARCHIVE_LIMIT');
      return storage.clear(namespace, keys);
    },
    async keys(namespace: string, entries: readonly EditionEntry[], context: (entry: EditionEntry) => unknown): Promise<Map<string, string>> {
      if (!/^[a-f0-9]{64}$/u.test(namespace) || entries.length > MAX_RECORDS) throw 'EDITIONS_ARCHIVE_LIMIT';
      const keys = new Map<string, string>();
      // Bounded digest batches yield to the renderer and avoid thousands of
      // simultaneous crypto requests. Only hashes reach persistent storage.
      for (let offset = 0; offset < entries.length; offset += 64) {
        const batch = await Promise.all(entries.slice(offset, offset + 64).map(async entry => [entry.key, namespace + ':' + await digest(editionArchiveFingerprint(entry, context(entry)))] as const));
        for (const [key, hash] of batch) keys.set(key, hash);
      }
      return keys;
    },
    async restore(keys: readonly string[]): Promise<Map<string, EditionProfile>> {
      if (keys.length > MAX_RECORDS || !keys.every(validKey)) throw 'EDITIONS_ARCHIVE_LIMIT';
      const stored = await storage.get(keys), result = new Map<string, EditionProfile>();
      for (const key of keys) { const cached = record(stored.get(key), key, now()); if (cached) result.set(key, cached.profile); }
      return result;
    },
    async save(key: string, profile: EditionProfile): Promise<void> {
      const cached = record({ key, at: now(), profile }, key, now());
      if (!cached) throw 'EDITIONS_ARCHIVE_RECORD';
      await storage.put(cached);
    },
  };
}

export function createIndexedEditionStorage(factory: () => IDBFactory = () => globalThis.indexedDB): EditionArchiveStorage {
  let opened: Promise<IDBDatabase> | null = null;
  const database = () => {
    if (opened) return opened;
    const pending = new Promise<IDBDatabase>((resolve, reject) => {
      let request: IDBOpenDBRequest;
      try { request = factory().open('sonante-album-editions-v1', 1); } catch { reject('EDITIONS_ARCHIVE_UNAVAILABLE'); return; }
      let expired = false;
      const timer = setTimeout(() => { expired = true; reject('EDITIONS_ARCHIVE_UNAVAILABLE'); }, 10000);
      request.onupgradeneeded = () => {
        const store = request.result.createObjectStore('profiles', { keyPath: 'key' }); store.createIndex('at', 'at');
        request.result.createObjectStore('usage').put({ count: 0, bytes: 0 }, 'total');
      };
      request.onerror = () => { clearTimeout(timer); reject('EDITIONS_ARCHIVE_UNAVAILABLE'); };
      request.onsuccess = () => {
        clearTimeout(timer);
        if (expired) { request.result.close(); return; }
        request.result.onversionchange = () => { request.result.close(); opened = null; };
        resolve(request.result);
      };
    });
    opened = pending;
    void pending.catch(() => { if (opened === pending) opened = null; });
    return pending;
  };
  const transaction = async <T>(mode: IDBTransactionMode, work: (tx: IDBTransaction, result: (value: T) => void) => void): Promise<T> => {
    const db = await database();
    return new Promise<T>((resolve, reject) => {
      const tx = db.transaction(['profiles', 'usage'], mode); let value: T;
      const timer = setTimeout(() => { try { tx.abort(); } catch { /* already complete */ } reject('EDITIONS_ARCHIVE_UNAVAILABLE'); }, 15000);
      tx.oncomplete = () => { clearTimeout(timer); resolve(value); };
      tx.onabort = tx.onerror = () => { clearTimeout(timer); reject('EDITIONS_ARCHIVE_UNAVAILABLE'); };
      try { work(tx, next => { value = next; }); } catch { tx.abort(); }
    });
  };
  return {
    get: keys => transaction<Map<string, unknown>>('readonly', (tx, finish) => {
      const values = new Map<string, unknown>(), store = tx.objectStore('profiles'); finish(values);
      for (const key of keys) { const request = store.get(key); request.onsuccess = () => { if (request.result) values.set(key, request.result); }; }
    }),
    clear: (namespace, keys) => transaction<void>('readwrite', (tx, finish) => {
      if (!/^[a-f0-9]{64}$/u.test(namespace) || keys && keys.some(key => !validKey(key) || !key.startsWith(namespace + ':'))) throw 'EDITIONS_ARCHIVE_LIMIT';
      const selected = keys ? new Set(keys) : null;
      const profiles = tx.objectStore('profiles'), usage = tx.objectStore('usage');
      let removed = 0, weight = 0;
      const cursor = profiles.openCursor();
      cursor.onsuccess = () => {
        const row = cursor.result;
        if (row) {
          if (String(row.key).startsWith(namespace + ':') && (!selected || selected.has(String(row.key)))) { removed++; weight += row.value.weight || 0; row.delete(); }
          row.continue(); return;
        }
        const stats = usage.get('total'); stats.onsuccess = () => {
          usage.put({ count: Math.max(0, (stats.result?.count || 0) - removed), bytes: Math.max(0, (stats.result?.bytes || 0) - weight) }, 'total'); finish();
        };
      };
    }),
    put: value => transaction<void>('readwrite', (tx, finish) => {
      const checked = record(value, value.key, value.at);
      if (!checked) throw 'EDITIONS_ARCHIVE_RECORD';
      value = checked;
      const profiles = tx.objectStore('profiles'), usage = tx.objectStore('usage');
      const stats = usage.get('total');
      stats.onsuccess = () => {
        const previous = profiles.get(value.key);
        previous.onsuccess = () => {
          let count = stats.result?.count, bytes = stats.result?.bytes;
          if (!Number.isSafeInteger(count) || !Number.isSafeInteger(bytes) || count < 0 || bytes < 0 || count > MAX_RECORDS || bytes > MAX_BYTES) {
            // This feature's disposable cache only; never user playlists/data.
            profiles.clear(); count = 0; bytes = 0;
          } else if (previous.result) { count--; bytes -= previous.result.weight || 0; }
          count++; bytes += value.weight;
          profiles.put(value);
          const done = () => { usage.put({ count, bytes }, 'total'); finish(); };
          if (count <= MAX_RECORDS && bytes <= MAX_BYTES) { done(); return; }
          const cursor = profiles.index('at').openCursor();
          cursor.onsuccess = () => {
            const row = cursor.result;
            if (!row || count <= MAX_RECORDS && bytes <= MAX_BYTES) { done(); return; }
            count--; bytes -= row.value.weight || 0; row.delete(); row.continue();
          };
        };
      };
    }),
  };
}
