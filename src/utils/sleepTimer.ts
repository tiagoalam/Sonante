export interface SleepTimerSnapshot { revision: number; phase: "idle" | "armed" | "expired" | "failed"; remaining_ms: number }
export interface SleepTimerView { timer: SleepTimerSnapshot | null; receivedAt: number; changing: boolean; error: boolean }
interface Transport {
  read: () => Promise<unknown>;
  set: (minutes: 15 | 30 | 60 | null) => Promise<unknown>;
  listen: (changed: (value: unknown) => void) => Promise<() => void>;
}
export function decodeSleepTimer(value: unknown): SleepTimerSnapshot {
  const timer = value as SleepTimerSnapshot | null;
  if (!timer || !Number.isSafeInteger(timer.revision) || timer.revision < 0 || !["idle", "armed", "expired", "failed"].includes(timer.phase)
    || !Number.isFinite(timer.remaining_ms) || timer.remaining_ms < 0 || timer.remaining_ms > 3600_000 || timer.phase !== "armed" && timer.remaining_ms !== 0) throw Error("SLEEP_TIMER_UNAVAILABLE");
  return { revision: timer.revision, phase: timer.phase, remaining_ms: timer.remaining_ms };
}
export const remainingSleepMinutes = (view: SleepTimerView, now: number) => view.timer?.phase === "armed"
  ? Math.max(0, Math.ceil((view.timer.remaining_ms - Math.max(0, now - view.receivedAt)) / 60_000)) : 0;
// One native subscription per WebView, shared by the player bar and expanded host.
export class SleepTimerStore {
  private view: SleepTimerView = { timer: null, receivedAt: 0, changing: false, error: false };
  private listeners = new Set<() => void>();
  private epoch = 0;
  private cleanup?: () => void;
  private transport: Transport;
  private now: () => number;
  constructor(transport: Transport, now: () => number) { this.transport = transport; this.now = now; }
  getSnapshot = () => this.view;
  private update(patch: Partial<SleepTimerView>) { this.view = { ...this.view, ...patch }; this.listeners.forEach(fn => fn()); }
  private accept(value: unknown) {
    const timer = decodeSleepTimer(value), previous = this.view.timer;
    if (previous && previous.revision > timer.revision) return;
    if (previous && previous.revision === timer.revision) {
      if (previous.phase !== timer.phase) return;
      // A delayed observation of the same deadline can never extend it.
      timer.remaining_ms = Math.min(timer.remaining_ms, Math.max(0, previous.remaining_ms - (this.now() - this.view.receivedAt)));
    }
    this.update({ timer, receivedAt: this.now(), error: false });
  }
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    if (this.listeners.size === 1) {
      const epoch = ++this.epoch;
      void this.transport.listen(value => { if (epoch === this.epoch) { try { this.accept(value); } catch { this.update({ error: true }); } } }).then(cleanup => {
        if (epoch !== this.epoch) { cleanup(); return; }
        this.cleanup = cleanup; void this.refresh();
      }).catch(() => { if (epoch === this.epoch) this.update({ error: true }); });
    }
    return () => { this.listeners.delete(fn); if (!this.listeners.size) { ++this.epoch; this.cleanup?.(); this.cleanup = undefined; } };
  };
  refresh = async () => {
    const epoch = this.epoch;
    try { const value = await this.transport.read(); if (epoch === this.epoch) this.accept(value); }
    catch { if (epoch === this.epoch) this.update({ error: true }); }
  };
  change = async (minutes: 15 | 30 | 60 | null) => {
    if (this.view.changing) return;
    this.update({ changing: true, error: false });
    try { this.accept(await this.transport.set(minutes)); }
    catch { this.update({ error: true }); }
    finally { this.update({ changing: false }); }
  };
}
