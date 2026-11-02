import type { ListeningSession } from '../types/listeningSession';
import { readSessions, writeSessions, SESSION_EVENT } from './listeningSessions';
import { sameSessionSelection } from './listeningSessionState';
export const listeningSessionStore = {
  list: () => readSessions(window.localStorage),
  save(session: ListeningSession) {
    const items = readSessions(window.localStorage);
    writeSessions(window.localStorage, [session, ...items.filter(item => item.id !== session.id)]);
    window.dispatchEvent(new Event(SESSION_EVENT));
  },
  updateProgress(session: ListeningSession, starting = false) {
    const items = readSessions(window.localStorage);
    const index = items.findIndex(item => sameSessionSelection(item, session));
    if (index < 0 || items[index].completed && !starting) return false;
    const stored = items[index];
    if (stored.cursor === session.cursor && stored.started_at === session.started_at && (!starting || !stored.completed)) return true;
    // Never insert, rename, reorder or resurrect a session during playback.
    items[index] = { ...stored, cursor: session.cursor, started_at: session.started_at, ...(starting ? { completed: false } : {}) };
    writeSessions(window.localStorage, items);
    window.dispatchEvent(new Event(SESSION_EVENT));
    return true;
  },
  delete(id: string) {
    writeSessions(window.localStorage, readSessions(window.localStorage).filter(item => item.id !== id));
    window.dispatchEvent(new Event(SESSION_EVENT));
  },
};
