interface FullscreenWindow { isFullscreen: () => Promise<boolean>; setFullscreen: (value: boolean) => Promise<void> }
interface FullscreenOptions {
  window: FullscreenWindow;
  changed: (value: boolean) => void;
  failed: (operation: "status" | "toggle", error: unknown) => void;
  restoreOnDispose: boolean;
}
// Window presentation only; no audio/host handoff commands. Main-window
// fullscreen entered here is released when the embedded player unmounts.
export class NowPlayingFullscreen {
  private options: FullscreenOptions;
  private disposed = false;
  private owned = false;
  private version = 0;
  private pending: Promise<void> | null = null;
  constructor(options: FullscreenOptions) { this.options = options; }
  async refresh(): Promise<void> {
    const version = ++this.version;
    try {
      const value = await this.options.window.isFullscreen();
      if (!this.disposed && version === this.version) this.options.changed(value);
    } catch (error) { if (!this.disposed) this.options.failed("status", error); }
  }
  toggle(): Promise<void> { return this.change(null); }
  exit(): Promise<void> { return this.change(false); }
  private change(requested: boolean | null): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.pending) return this.pending;
    ++this.version;
    const operation = (async () => {
      const value = requested ?? !(await this.options.window.isFullscreen());
      if (this.disposed) return;
      await this.options.window.setFullscreen(value);
      this.owned = value;
      if (!this.disposed) this.options.changed(value);
    })().catch(error => this.options.failed("toggle", error));
    this.pending = operation.finally(() => { this.pending = null; });
    return this.pending;
  }
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true; ++this.version;
    if (!this.options.restoreOnDispose) return;
    // A native entry already in flight must settle before restoring; closing
    // the host while the native query is pending never enters fullscreen.
    void (this.pending ?? Promise.resolve()).then(async () => {
      if (!this.owned) return;
      this.owned = false;
      try { await this.options.window.setFullscreen(false); }
      catch (error) { this.options.failed("toggle", error); }
    });
  }
}
