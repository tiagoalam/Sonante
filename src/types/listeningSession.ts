import type { SmartSource, SmartAlbum } from './smartPlaylist';
export type SessionMode = 'discovery' | 'recent' | 'unplayed';
export interface SessionCandidate extends SmartAlbum { scope_id?: string; scope_label?: string }
export interface SessionAlbum {
  id: string; title: string; artist: string; year: number | null;
  duration: number | null; keys: string[]; reason: SessionMode | 'selected';
}
export interface ListeningSession {
  id: string; name: string; created_at: number; source: SmartSource;
  started_at?: number; target: number; albums: SessionAlbum[]; cursor: number; completed: boolean;
}
