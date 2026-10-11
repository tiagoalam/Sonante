import type { ConfigSaveOperation } from "../types/config.ts";
import type { LocalImportPreview, PreviewAlbum } from "../types/localImportPreview.ts";

export const LOCAL_IMPORT_PREVIEW_LIMIT = 48;
export const LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES = 512 * 1024;

/** Bounded embedded PNG only; browser decoding failures still use the placeholder. */
export function safeImportPreviewImage(value: unknown): string | null {
  const prefix = "data:image/png;base64,";
  if (typeof value !== "string" || !value.startsWith(prefix)
    || value.length > prefix.length + 4 * Math.ceil(LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES / 3)) return null;
  const data = value.slice(prefix.length);
  if (data.length < 12 || data.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(data)) return null;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  if (data.length / 4 * 3 - padding > LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES) return null;
  // Decode only the signature, avoiding a second full-size image allocation.
  return atob(data.slice(0, 12)).slice(0, 8) === "\x89PNG\r\n\x1a\n" ? value : null;
}

export function filterImportPreview(albums: readonly PreviewAlbum[], source: string | null, search = ""): PreviewAlbum[] {
  const query = search.toLowerCase();
  return albums.filter(album => (source === null || album.source_id === source)
    && `${album.album ?? ""}\n${album.artist ?? ""}`.toLowerCase().includes(query));
}

/** Driven solely by the Save monitor; no timer, catalog writes or media reads. */
export function createLocalImportPreview(
  read: (operationId: string) => Promise<LocalImportPreview>,
  publish: (albums: PreviewAlbum[]) => void,
) {
  let operationId: string | null = null;
  let generation = 0;
  let disposed = false;
  let reading = false;
  const albums = new Map<string, PreviewAlbum>();
  const clear = () => {
    generation++;
    operationId = null;
    albums.clear();
    if (!disposed) publish([]);
  };
  return {
    clear,
    observe(operation: ConfigSaveOperation) {
      if (disposed) return;
      if (operation.phase !== "indexing") {
        if (operationId !== null) clear();
        return;
      }
      if (operationId !== operation.id) {
        clear();
        operationId = operation.id;
      }
      if (reading) return;
      reading = true;
      const id = operation.id;
      const requestGeneration = generation;
      void (async () => {
        try {
          const sample = await read(id);
          if (disposed || generation !== requestGeneration || operationId !== id
            || sample.operation_id !== id || sample.provisional !== true
            || sample.playable !== false || sample.complete !== false
            || !Array.isArray(sample.albums) || sample.albums.length > 128) return;
          for (const row of sample.albums) {
            if (!/^[a-f0-9]{64}$/.test(row.provisional_group_key)
              || typeof row.source_id !== "string" || row.source_id.length > 256
              || (row.album !== null && (typeof row.album !== "string" || row.album.length > 256))
              || (row.artist !== null && (typeof row.artist !== "string" || row.artist.length > 256))) continue;
            // Copy only presentation fields, never arbitrary response properties.
            albums.set(row.provisional_group_key, {
              provisional_group_key: row.provisional_group_key,
              source_id: row.source_id, album: row.album, artist: row.artist,
              image: safeImportPreviewImage(row.image) ?? albums.get(row.provisional_group_key)?.image ?? null,
            });
            if (albums.size > LOCAL_IMPORT_PREVIEW_LIMIT) albums.delete(albums.keys().next().value!);
          }
          publish([...albums.values()]);
        } catch {
          // Busy/Unavailable are expected. Other preview read failures also do
          // not change Save's outcome; retry only on the next existing poll.
        } finally {
          reading = false;
        }
      })();
    },
    dispose() {
      disposed = true;
      clear();
    },
  };
}
