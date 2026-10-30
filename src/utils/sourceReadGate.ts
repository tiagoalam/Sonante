// Main-window source readers share one physical IPC slot. Player facts take
// priority over cover decoration. At most one queued job of each kind.
type Job = { kind: "player" | "album"; current: () => boolean;
  read: () => Promise<unknown>; resolve: (value: unknown) => void };
export class SourceReadGate {
  private pending = false;
  private queue = new Map<Job["kind"], Job>();
  run<T>(kind: Job["kind"], read: () => Promise<T | null>, current: () => boolean): Promise<T | null> {
    return new Promise<T | null>(resolve => {
      this.queue.get(kind)?.resolve(null);
      this.queue.set(kind, { kind, current, read, resolve: value => resolve(value as T | null) });
      void Promise.resolve().then(() => this.pump());
    });
  }
  private pump() {
    if (this.pending) return;
    const job = this.queue.get("player") ?? this.queue.get("album");
    if (!job) return;
    this.queue.delete(job.kind);
    if (!job.current()) { job.resolve(null); this.pump(); return; }
    this.pending = true;
    void Promise.resolve().then(job.read).catch(() => null).then(value => job.resolve(job.current() ? value : null))
      .finally(() => { this.pending = false; this.pump(); });
  }
}
