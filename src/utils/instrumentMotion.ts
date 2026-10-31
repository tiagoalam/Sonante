import type { AudioLevelFrame } from "../types/audio";
export const INSTRUMENT_BANDS = 48;
export const unitLevel = (v: number) => Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 0;
export const levelDbfs = (v: number) => v > 0 && Number.isFinite(v) ? Math.max(-90, Math.min(0, 20 * Math.log10(v))) : -90;
export const meterPosition = (v: number, floor = -48) => Math.max(0, Math.min(1, (levelDbfs(v) - floor) / -floor));
export interface InstrumentFrame {
  rms: number[];
  peak: number[];
  spectrum: number[];
  spectrumPeaks: number[];
}
const empty = (): InstrumentFrame => ({ rms: [0, 0], peak: [0, 0], spectrum: Array(INSTRUMENT_BANDS).fill(0), spectrumPeaks: Array(INSTRUMENT_BANDS).fill(0) });

// One latest target and one time-based motion model for the complete instrument.
// This affects drawing only; never creates an audio clock, sample delay or queue.
export class InstrumentMotion {
  readonly frame = empty();
  private target = empty();
  private holds = Array(INSTRUMENT_BANDS).fill(0);
  setTarget(levels: AudioLevelFrame, active: boolean) {
    this.target.rms[0] = active ? unitLevel(levels.leftRms) : 0;
    this.target.rms[1] = active ? unitLevel(levels.rightRms) : 0;
    this.target.peak[0] = active ? Math.max(this.target.rms[0], unitLevel(levels.leftPeak)) : 0;
    this.target.peak[1] = active ? Math.max(this.target.rms[1], unitLevel(levels.rightPeak)) : 0;
    for (let i = 0; i < INSTRUMENT_BANDS; i++) this.target.spectrum[i] = active ? unitLevel(levels.spectrum[i] ?? 0) : 0;
  }
  step(milliseconds: number, snap = false): boolean {
    const dt = Number.isFinite(milliseconds) ? Math.max(0, Math.min(250, milliseconds)) : 16.67;
    // Fast attack removes the previous stacked visual delays. Release is intentionally slower.
    const smooth = (current: number, target: number) => {
      const value = snap ? target : current + (target - current) * (1 - Math.exp(-dt / (target > current ? 10 : 135)));
      return Math.abs(target - value) < .0005 ? target : value;
    };
    let moving = false;
    for (let i = 0; i < 2; i++) {
      this.frame.rms[i] = smooth(this.frame.rms[i], this.target.rms[i]);
      this.frame.peak[i] = snap ? this.target.peak[i] : Math.max(this.target.peak[i], this.frame.peak[i] - dt * .002);
      moving ||= this.frame.rms[i] !== this.target.rms[i] || this.frame.peak[i] !== this.target.peak[i];
    }
    for (let i = 0; i < INSTRUMENT_BANDS; i++) {
      const target = this.target.spectrum[i];
      this.frame.spectrum[i] = smooth(this.frame.spectrum[i], target);
      if (snap || target >= this.frame.spectrumPeaks[i]) {
        this.frame.spectrumPeaks[i] = target; this.holds[i] = snap ? 0 : 180;
      } else {
        this.holds[i] = Math.max(0, this.holds[i] - dt);
        if (this.holds[i] === 0) this.frame.spectrumPeaks[i] = Math.max(target, this.frame.spectrumPeaks[i] - dt * .001);
      }
      moving ||= this.frame.spectrum[i] !== target || this.frame.spectrumPeaks[i] > target + .0005;
    }
    return moving;
  }
}
