import type { AnalyzerOwner } from "../types/audio";

export interface HostingState {
  mode: "closed" | "embedded" | "detached";
  owner: AnalyzerOwner | null;
  busy: boolean;
  error: boolean;
}
export interface HostingServices {
  claim: (host: AnalyzerOwner["host"], session: string) => Promise<AnalyzerOwner>;
  open: () => Promise<string>;
  activate: (runtime: string, owner: AnalyzerOwner) => Promise<void>;
  suspend: () => Promise<void>;
  close: () => Promise<void>;
  stop: (owner: AnalyzerOwner) => Promise<void>;
  id: () => string;
  beforeEmbed?: () => void;
}

// main is the single transition authority. No transport/queue commands here.
export class NowPlayingHosting {
  private state: HostingState = { mode: "closed", owner: null, busy: false, error: false };
  private services: HostingServices;
  private listeners = new Set<() => void>();
  private ready = new Map<number, () => void>();
  private runtime: string | null = null;
  private closedDuringTransition = false;
  constructor(services: HostingServices) { this.services = services; }
  getSnapshot = () => this.state;
  subscribe = (fn: () => void) => { this.listeners.add(fn); return () => { this.listeners.delete(fn); }; };
  private update(value: Partial<HostingState>) { this.state = { ...this.state, ...value }; this.listeners.forEach(fn => fn()); }
  isDetachedRuntime = (runtime: string) => this.state.mode === "detached" && !this.state.busy && this.runtime === runtime;
  hostReady = (owner: AnalyzerOwner) => { this.ready.get(owner.generation)?.(); };
  private async embed() {
    if (this.state.mode !== "embedded") this.services.beforeEmbed?.();
    const owner = await this.services.claim("main", this.services.id());
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => { this.ready.delete(owner.generation); reject(new Error("Host readiness timeout")); }, 8000);
      this.ready.set(owner.generation, () => { clearTimeout(timeout); this.ready.delete(owner.generation); resolve(); });
      this.update({ mode: "embedded", owner });
    });
  }
  show = async () => {
    if (this.state.busy || this.state.mode === "embedded") return;
    if (this.state.mode === "detached") return this.reincorporate();
    this.update({ busy: true, error: false });
    try { await this.embed(); } catch { this.update({ error: true }); }
    finally { this.update({ busy: false }); }
  };
  collapse = async () => {
    if (this.state.busy || this.state.mode !== "embedded") return;
    this.update({ busy: true });
    try {
      if (this.state.owner) await this.services.stop(this.state.owner);
      this.update({ mode: "closed", owner: null, error: false });
    } catch { this.update({ error: true }); }
    finally { this.update({ busy: false }); }
  };
  detach = async () => {
    if (this.state.busy || this.state.mode !== "embedded") return;
    this.update({ busy: true, error: false });
    let transferred = false;
    this.closedDuringTransition = false;
    try {
      const runtime = await this.services.open();
      const owner = await this.services.claim("now-playing", this.services.id());
      transferred = true;
      await this.services.activate(runtime, owner);
      if (this.closedDuringTransition) throw new Error("Detached host closed during activation");
      this.runtime = runtime;
      this.update({ mode: "detached", owner: null });
    } catch {
      if (transferred) {
        await this.services.suspend().catch(() => {});
        try { await this.embed(); } catch { /* Visible embedded error remains; never send transport commands. */ }
      }
      await this.services.close().catch(() => {});
      this.update({ error: true });
    } finally { this.update({ busy: false }); }
  };
  reincorporate = async () => {
    if (this.state.busy || this.state.mode !== "detached" || !this.runtime) return;
    this.update({ busy: true, error: false });
    try {
      await this.services.suspend();
      await this.embed();
      await this.services.close();
      this.runtime = null;
    } catch {
      // Roll back to the confirmed detached host if closing/activation failed.
      try {
        const owner = await this.services.claim("now-playing", this.services.id());
        await this.services.activate(this.runtime!, owner);
        this.update({ mode: "detached", owner: null });
      } catch {
        try { await this.embed(); } catch { /* Keep a reviewable error in the current host. */ }
      }
      this.update({ error: true });
    } finally { this.update({ busy: false }); }
  };
  detachedClosed = () => {
    this.closedDuringTransition = true;
    if (!this.state.busy && this.state.mode === "detached") {
      this.runtime = null;
      this.update({ mode: "closed", owner: null });
    }
  };
}

export const analyzerOwnerMatches = (a: AnalyzerOwner | null, b: AnalyzerOwner) =>
  !!a && a.host === b.host && a.sessionId === b.sessionId && a.generation === b.generation && (!a.leaseId || a.leaseId === b.leaseId);

// Keep the actual page mounted: restoring scroll/focus must not reload it.
export const capturePagePresentation = (root: HTMLElement, active: Element | null) => {
  const positions = [root, ...root.querySelectorAll<HTMLElement>("*")]
    .filter(node => node.scrollTop || node.scrollLeft).map(node => ({ node, top: node.scrollTop, left: node.scrollLeft }));
  const focus = active instanceof HTMLElement ? active : null;
  return () => {
    positions.forEach(({ node, top, left }) => { if (node.isConnected) { node.scrollTop = top; node.scrollLeft = left; } });
    if (focus?.isConnected && !focus.closest("[hidden]")) focus.focus({ preventScroll: true });
  };
};
