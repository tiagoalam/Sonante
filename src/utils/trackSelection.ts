import type { TrackMetadata } from "../types/audio";
export function selectionIdentity(tracks: TrackMetadata[]): string { return JSON.stringify(tracks.map(track => track.media_locator ?? { kind: "local", uri: track.uri })); }
export function changeSelection(selected: ReadonlySet<number>, index: number, anchor: number | null, range: boolean): Set<number> {
  const next = new Set(selected);
  if (range && anchor !== null) { for (let i = Math.min(anchor, index); i <= Math.max(anchor, index); i++) next.add(i); }
  else if (next.has(index)) next.delete(index); else next.add(index);
  return next;
}
export function selectedTracks(tracks: TrackMetadata[], selected: ReadonlySet<number>): TrackMetadata[] {
  return tracks.filter((_, index) => selected.has(index));
}
