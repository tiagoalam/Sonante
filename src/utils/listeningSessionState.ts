import type { ListeningSession } from '../types/listeningSession';

// Names, timestamps and cursors are editable metadata. Progress belongs to this
// exact source and ordered album/track selection, never to a name or an id alone.
export function sameSessionSelection(a: ListeningSession | null, b: ListeningSession | null): boolean {
  if (!a || !b) return false;
  const source = a.source, other = b.source;
  const sameSource = source.kind === 'local' ? other.kind === 'local' : source.kind === 'plex'
    ? other.kind === 'plex' && source.server_id === other.server_id && source.library_id === other.library_id
    : other.kind === 'navidrome' && source.account_id === other.account_id;
  return sameSource && a.id === b.id
    && a.albums.length === b.albums.length && a.albums.every((album, i) => album.id === b.albums[i].id
      && album.keys.length === b.albums[i].keys.length && album.keys.every((key, j) => key === b.albums[i].keys[j]));
}

export function resolveSessionSelection(draft: ListeningSession | null, saved: readonly ListeningSession[], playing: ListeningSession | null): ListeningSession | null {
  if (!draft) return null;
  const value = saved.find(item => sameSessionSelection(item, draft)) ?? draft;
  return !value.completed && sameSessionSelection(value, playing)
    ? { ...value, cursor: playing!.cursor, started_at: playing!.started_at } : value;
}

export function sessionDisplayName(name: string, fallback: string, saved: readonly ListeningSession[], id?: string): string {
  const base = (name.trim() || fallback.trim()).slice(0, 120);
  const names = new Set(saved.filter(item => item.id !== id).map(item => item.name));
  let value = base;
  for (let n = 2; names.has(value); n++) {
    const suffix = ` (${n})`; value = base.slice(0, 120 - suffix.length) + suffix;
  }
  return value;
}
