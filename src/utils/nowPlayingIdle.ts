interface IdleOptions {
  setTimer: (callback: () => void, milliseconds: number) => number;
  clearTimer: (timer: number) => void;
  changed: (hidden: boolean) => void;
  blocked: () => boolean;
  delay?: number;
}
// One UI-only timeout, never a polling loop or a playback timer.
export class NowPlayingIdle {
  private options: IdleOptions;
  private timer: number | undefined;
  private disposed = false;
  hidden = false;
  constructor(options: IdleOptions) { this.options = options; }
  private clear() {
    if (this.timer !== undefined) this.options.clearTimer(this.timer);
    this.timer = undefined;
  }
  private show() {
    if (this.hidden) { this.hidden = false; this.options.changed(false); }
  }
  activity() {
    if (this.disposed) return;
    this.clear(); this.show();
    this.timer = this.options.setTimer(() => {
      this.timer = undefined;
      if (!this.disposed && !this.options.blocked()) { this.hidden = true; this.options.changed(true); }
    }, this.options.delay ?? 4000);
  }
  pause() { if (!this.disposed) { this.clear(); this.show(); } }
  dispose() { this.disposed = true; this.clear(); }
}
