import { AB_RATE, abLevels, type ABSample, type ABSide } from './albumAB';
// Both buffers share ONE AudioContext and start time. Switching never seeks,
// starts another player or reopens output. A 5 ms linear ramp limits clicks.
export class AlbumABGraph {
  private sources: AudioBufferSourceNode[] = [];
  private gains: GainNode[] = [];
  private master: GainNode;
  private closed = false;
  private startedAt: number | null = null;
  private side: ABSide = 'a';
  private levels: [number, number];
  private matched: [number, number];
  private original: [number, number];
  private ramp = { at: 0, from: [0, 0], to: [0, 0] };
  readonly duration: number;
  constructor(private context: AudioContext, a: ABSample, b: ABSample, match: boolean, volume: number) {
    this.duration = Math.min(a.frames, b.frames) / AB_RATE;
    this.matched = abLevels(a, b, true); this.original = abLevels(a, b, false); this.levels = match ? this.matched : this.original;
    this.master = context.createGain(); this.master.gain.value = Math.max(0, Math.min(1, volume)); this.master.connect(context.destination);
    for (const sample of [a, b]) {
      const buffer = context.createBuffer(2, Math.min(a.frames, b.frames), AB_RATE);
      buffer.copyToChannel(sample.left.subarray(0, buffer.length), 0); buffer.copyToChannel(sample.right.subarray(0, buffer.length), 1);
      const source = context.createBufferSource(), gain = context.createGain();
      source.buffer = buffer; source.loop = true; source.loopStart = 0; source.loopEnd = this.duration; gain.gain.value = 0;
      source.connect(gain); gain.connect(this.master); this.sources.push(source); this.gains.push(gain);
    }
  }
  async unlock() { if (this.closed) throw 'AB_CHANGED'; await this.context.resume(); if (this.closed || this.context.state !== 'running') throw 'AB_UNAVAILABLE'; }
  start(side: ABSide) {
    if (this.closed || this.startedAt !== null) throw 'AB_CHANGED';
    this.side = side; this.startedAt = this.context.currentTime + 0.05;
    const to = side === 'a' ? [this.levels[0], 0] : [0, this.levels[1]];
    this.ramp = { at: this.startedAt, from: to, to };
    this.gains.forEach((gain, i) => gain.gain.setValueAtTime(to[i], this.startedAt!));
    this.sources.forEach(source => source.start(this.startedAt!, 0));
  }
  private current(at: number): number[] { const p = Math.max(0, Math.min(1, (at - this.ramp.at) / 0.005)); return this.ramp.from.map((v, i) => v + (this.ramp.to[i] - v) * p); }
  select(side: ABSide) {
    if (this.closed) return; this.side = side;
    const at = this.context.currentTime, from = this.current(at), to = side === 'a' ? [this.levels[0], 0] : [0, this.levels[1]];
    this.gains.forEach((gain, i) => { gain.gain.cancelScheduledValues(at); gain.gain.setValueAtTime(from[i], at); gain.gain.linearRampToValueAtTime(to[i], at + 0.005); });
    this.ramp = { at, from, to };
  }
  settings(match: boolean, volume: number) { this.levels = match ? this.matched : this.original; this.master.gain.setTargetAtTime(Math.max(0, Math.min(1, volume)), this.context.currentTime, 0.01); this.select(this.side); }
  elapsed(): number { return this.startedAt === null ? 0 : Math.max(0, this.context.currentTime - this.startedAt) % this.duration; }
  async pause() { if (!this.closed) await this.context.suspend(); }
  async resume() { if (!this.closed) await this.context.resume(); }
  close() {
    if (this.closed) return; this.closed = true;
    this.sources.forEach(source => { try { source.stop(); } catch { /* Not started. */ } source.disconnect(); });
    this.gains.forEach(gain => gain.disconnect()); this.master.disconnect(); void this.context.close().catch(() => {});
  }
}
