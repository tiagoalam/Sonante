import { editionProfile, type EditionProfile } from '../../src/utils/albumEditionAnalysis';
import type { EditionEntry } from '../../src/utils/albumEditions';

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
  const fail = 'EDITIONS_ARCHIVE_UNAVAILABLE';
  const database = () => {
    if (opened) return opened;
    const pending = new Promise<IDBDatabase>((resolve, reject) => {
      let request: IDBOpenDBRequest;
      try { request = factory().open('sonante-album-editions-v1', 1); } catch { reject(fail); return; }
      let expired = false;
      const timer = setTimeout(() => { expired = true; reject(fail); }, 10000);
      request.onupgradeneeded = event => {
        if (expired || event.oldVersion !== 0) { request.transaction?.abort(); return; }
        const store = request.result.createObjectStore('profiles', { keyPath: 'key' }); store.createIndex('at', 'at');
        request.result.createObjectStore('usage').put({ count: 0, bytes: 0 }, 'total');
      };
      request.onerror = () => { clearTimeout(timer); reject(fail); };
      request.onsuccess = () => {
        clearTimeout(timer);
        if (expired) { request.result.close(); return; }
        const db = request.result;
        if (!db.objectStoreNames.contains('profiles') || !db.objectStoreNames.contains('usage')) { db.close(); reject(fail); return; }
        db.onversionchange = () => { db.close(); opened = null; }; resolve(db);
      };
    });
    opened = pending;
    void pending.catch(() => { if (opened === pending) opened = null; });
    return pending;
  };
  const transaction = async <T>(mode: IDBTransactionMode, work: (tx: IDBTransaction, rows: Map<string, EditionArchiveRecord>, result: (value: T) => void) => void): Promise<T> => {
    const db = await database();
    return new Promise<T>((resolve, reject) => {
      let tx: IDBTransaction;
      try { tx = db.transaction(['profiles', 'usage'], mode); } catch { reject(fail); return; }
      let value: T;
      const abort = () => { try { tx.abort(); } catch { /* already terminal */ } reject(fail); };
      const timer = setTimeout(abort, 15000);
      tx.oncomplete = () => { clearTimeout(timer); resolve(value); };
      tx.onabort = tx.onerror = () => { clearTimeout(timer); reject(fail); };
      // Validate the complete bounded inventory and accounting BEFORE any
      // mutation. An invalid/unknown row or stats record is never repaired,
      // replaced or removed implicitly, including on explicit reanalysis.
      try {
        const profiles = tx.objectStore('profiles'), usage = tx.objectStore('usage');
        if (profiles.keyPath !== 'key' || usage.keyPath !== null || !profiles.indexNames.contains('at') || profiles.index('at').keyPath !== 'at') { abort(); return; }
        const stats = usage.get('total');
        stats.onsuccess = () => {
          try {
            const count = stats.result?.count, bytes = stats.result?.bytes;
            if (!Number.isSafeInteger(count) || !Number.isSafeInteger(bytes) || count < 0 || bytes < 0 || count > MAX_RECORDS || bytes > MAX_BYTES) { abort(); return; }
            const rows = new Map<string, EditionArchiveRecord>(); let weight = 0;
            const cursor = profiles.openCursor();
            cursor.onsuccess = () => {
              try {
                const row = cursor.result;
                if (row) {
                  const key = row.key, raw = row.value;
                  if (!validKey(key) || !raw || !Number.isSafeInteger(raw.at) || raw.at < 0 || raw.at > Date.now()) { abort(); return; }
                  const checked = record(raw, key, raw.at);
                  if (!checked || checked.weight !== raw.weight || rows.has(key)) { abort(); return; }
                  rows.set(key, checked); weight += checked.weight;
                  if (rows.size > MAX_RECORDS || weight > MAX_BYTES) { abort(); return; }
                  row.continue(); return;
                }
                if (count !== rows.size || bytes !== weight) { abort(); return; }
                work(tx, rows, next => { value = next; });
              } catch { abort(); }
            };
          } catch { abort(); }
        };
      } catch { abort(); }
    });
  };
  const accounting = (tx: IDBTransaction, rows: Map<string, EditionArchiveRecord>) => {
    tx.objectStore('usage').put({ count: rows.size, bytes: [...rows.values()].reduce((n, row) => n + row.weight, 0) }, 'total');
  };
  return {
    get: keys => transaction('readonly', (_tx, rows, finish) => {
      if (keys.length > MAX_RECORDS || !keys.every(validKey)) throw fail;
      finish(new Map(keys.flatMap(key => rows.has(key) ? [[key, rows.get(key)]] : [])));
    }),
    clear: (namespace, keys) => transaction<void>('readwrite', (tx, rows, finish) => {
      if (!/^[a-f0-9]{64}$/u.test(namespace) || keys && (keys.length > MAX_RECORDS || keys.some(key => !validKey(key) || !key.startsWith(namespace + ':')))) throw fail;
      const selected = keys ? new Set(keys) : null, profiles = tx.objectStore('profiles');
      for (const key of rows.keys()) if (key.startsWith(namespace + ':') && (!selected || selected.has(key))) { profiles.delete(key); rows.delete(key); }
      accounting(tx, rows); finish();
    }),
    put: value => transaction<void>('readwrite', (tx, rows, finish) => {
      const checked = value && record(value, value.key, value.at);
      if (!checked || checked.weight !== value.weight) throw fail;
      const profiles = tx.objectStore('profiles'); rows.set(checked.key, checked); profiles.put(checked);
      let bytes = [...rows.values()].reduce((n, row) => n + row.weight, 0);
      // Original healthy-cache bounded eviction policy; never a reset/repair.
      const oldest = [...rows.values()].sort((a, b) => a.at - b.at || a.key.localeCompare(b.key));
      for (const row of oldest) {
        if (rows.size <= MAX_RECORDS && bytes <= MAX_BYTES) break;
        rows.delete(row.key); profiles.delete(row.key); bytes -= row.weight;
      }
      accounting(tx, rows); finish();
    }),
  };
}
