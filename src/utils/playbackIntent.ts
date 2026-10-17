// One preparation per admitted gesture. Backend tokens coordinate all windows;
// the local generation also fences work before its first IPC/HTTP await.
export interface PlaybackIntent {
  token: Promise<string | null>;
  current(): boolean;
  confirmCurrent(): Promise<boolean>;
  cancel(): void;
}
let admission: () => Promise<string | null> = () => Promise.resolve(null);
let cancellation: (token: string) => Promise<unknown> = async () => {};
let confirmation: (token: string) => Promise<boolean> = async () => true;
let latest: PlaybackIntent | undefined;
export function configurePlaybackIntents(admit: () => Promise<string>, cancel: (token: string) => Promise<unknown>, confirm: (token: string) => Promise<boolean> = async () => true) {
  admission = admit; cancellation = cancel; confirmation = confirm;
}
export function beginPlaybackIntent(): PlaybackIntent {
  latest?.cancel();
  let cancelled = false;
  const token = admission();
  // Admission may fail before a slow preparation reaches its await. The same
  // rejection is still propagated when consumed; never leave it unhandled.
  void token.catch(() => {});
  const intent: PlaybackIntent = { token, current: () => !cancelled && latest === intent,
    async confirmCurrent() {
      let value: string | null;
      try { value = await token; } catch { return intent.current(); }
      if (!intent.current()) return false;
      try { return (value === null || await confirmation(value)) && intent.current(); }
      catch { return false; }
    },
    cancel() {
      if (cancelled) return;
      cancelled = true;
      void token.then(value => value === null ? undefined : cancellation(value)).catch(() => {});
    } };
  latest = intent;
  return intent;
}
export const isPlaybackSuperseded = (error: unknown): boolean => error === "PLAYBACK_SUPERSEDED";
export async function playbackIntentToken(intent: PlaybackIntent): Promise<string | null> {
  const token = await intent.token;
  if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
  return token;
}
export type PlaybackFailure = "uncertain" | "persistence" | "failed";
export function playbackFailure(error: unknown): PlaybackFailure {
  return error === "PLAYBACK_QUEUE_UNCERTAIN" ? "uncertain"
    : error === "PLAYBACK_PERSISTENCE_FAILED" ? "persistence" : "failed";
}
