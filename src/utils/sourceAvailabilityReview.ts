import type { LocalSourceAvailabilityObservation } from "../types/local";

export interface SourceAvailabilityReviewState {
  context: object | null;
  phase: "idle" | "loading" | "observed" | "failed";
  pending: boolean;
  observation: LocalSourceAvailabilityObservation | null;
}

// Finite informational observations only. No automatic I/O, timers, retries,
// cancellation claim, catalog mutation or read/playback capability is exposed.
export class SourceAvailabilityReview {
  private context: object | null = null;
  private revision = 0;
  private pending = false;
  private listeners = new Set<() => void>();
  private state: SourceAvailabilityReviewState = {
    context: null, phase: "idle", pending: false, observation: null,
  };
  private read: () => Promise<LocalSourceAvailabilityObservation>;
  constructor(read: () => Promise<LocalSourceAvailabilityObservation>) { this.read = read; }
  getSnapshot = (): SourceAvailabilityReviewState => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private publish(phase: SourceAvailabilityReviewState["phase"], observation: LocalSourceAvailabilityObservation | null = null) {
    this.state = { context: this.context, phase, pending: this.pending, observation };
    this.listeners.forEach(listener => listener());
  }
  setContext(context: object | null) {
    if (context === this.context) return;
    this.context = context;
    this.revision += 1;
    this.publish(this.pending ? "loading" : "idle");
  }
  async check(): Promise<void> {
    if (this.pending || this.context === null) return;
    const revision = this.revision;
    const context = this.context;
    this.pending = true;
    this.publish("loading");
    let observation: LocalSourceAvailabilityObservation | null = null;
    try { observation = await this.read(); } catch { /* Show a localized, manual failure. */ }
    this.pending = false;
    if (revision !== this.revision || context !== this.context) {
      this.publish("idle");
      return;
    }
    this.publish(observation ? "observed" : "failed", observation);
  }
}
