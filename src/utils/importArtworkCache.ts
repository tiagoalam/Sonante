import { safeImportPreviewImage } from "./localImportPreview.ts";

// PNG payload only; metadata/DOM and one bounded IPC reply are separate costs.
// Sixteen maximum-size (512 KiB) covers fit; ordinary thumbnails fit many more.
export const IMPORT_ARTWORK_CACHE_BYTES = 8 * 1024 * 1024;
export interface ImportArtworkCache {
  activate(id: string): void;
  pause(): void;
  clear(): void;
  tick(): void;
  accept(reference: string, image: unknown): void;
  get(reference: string): string | null;
  watch(reference: string, changed: () => void): () => void;
}

export function createImportArtworkCache(
  read: (id: string, reference: string) => Promise<string | null>,
  options: {
    budget?: number;
    create?: (data: string) => { url: string; bytes: number };
    revoke?: (url: string) => void;
  } = {},
): ImportArtworkCache & { retainedBytes(): number } {
  const budget = options.budget ?? IMPORT_ARTWORK_CACHE_BYTES;
  const revoke = options.revoke ?? URL.revokeObjectURL.bind(URL);
  const create = options.create ?? ((data: string) => {
    const raw = atob(data.slice(data.indexOf(",") + 1));
    const bytes = Uint8Array.from(raw, c => c.charCodeAt(0));
    return { url: URL.createObjectURL(new Blob([bytes], { type: "image/png" })), bytes: bytes.length };
  });
  const entries = new Map<string, { url: string; bytes: number }>();
  const visible = new Map<string, Set<() => void>>();
  const tried = new Set<string>();
  let id: string | null = null, epoch = 0, active = false, busy = false, attempts = 0, bytes = 0;
  const valid = (reference: string) => id !== null && reference.startsWith(`cover-v1:${id}:`)
    && reference.length <= 8220;
  const notify = (reference: string) => visible.get(reference)?.forEach(changed => changed());
  const remove = (reference: string) => {
    const entry = entries.get(reference);
    if (!entry) return;
    entries.delete(reference); bytes -= entry.bytes;
    revoke(entry.url); notify(reference);
  };
  const accept = (reference: string, value: unknown) => {
    if (!valid(reference) || !visible.has(reference)) return;
    const data = safeImportPreviewImage(value);
    if (!data) return;
    const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
    const size = (data.length - data.indexOf(",") - 1) / 4 * 3 - padding;
    if (size > budget) return;
    remove(reference);
    while (bytes + size > budget && entries.size) remove(entries.keys().next().value!);
    let entry: { url: string; bytes: number };
    try { entry = create(data); } catch { return; }
    if (entry.bytes !== size) { revoke(entry.url); return; }
    entries.set(reference, entry); bytes += entry.bytes; notify(reference);
  };
  const pump = async () => {
    if (!active || busy || attempts === 0 || id === null) return;
    const reference = [...visible.keys()].find(key => valid(key) && !entries.has(key) && !tried.has(key));
    if (!reference) return;
    const requestEpoch = epoch, operation = id;
    busy = true; attempts--; tried.add(reference);
    try {
      const image = await read(operation, reference);
      if (active && epoch === requestEpoch && id === operation) accept(reference, image);
    } catch { /* Optional cover: retry on the next existing monitor tick. */ }
    finally { busy = false; void pump(); }
  };
  const clear = () => {
    epoch++; active = false; id = null; attempts = 0; tried.clear();
    for (const reference of [...entries.keys()]) remove(reference);
  };
  return {
    activate(next) {
      if (id !== next) { clear(); id = next; }
      active = true;
    },
    pause() { if (active) epoch++; active = false; attempts = 0; },
    clear,
    tick() { tried.clear(); attempts = 2; void pump(); },
    accept,
    get(reference) {
      const entry = entries.get(reference);
      if (!entry) return null;
      entries.delete(reference); entries.set(reference, entry); // LRU by visible use.
      return entry.url;
    },
    watch(reference, changed) {
      let listeners = visible.get(reference);
      if (!listeners) { listeners = new Set(); visible.set(reference, listeners); }
      listeners.add(changed); void pump();
      return () => { listeners!.delete(changed); if (!listeners!.size) visible.delete(reference); };
    },
    retainedBytes: () => bytes,
  };
}
