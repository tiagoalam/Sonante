import SyncWorker from '../workers/albumABSync.worker?worker';
import { AB_RATE, type ABSample } from '../utils/albumAB';
import type { ABAlignment } from '../utils/albumABSync';

const ANALYSIS_FRAMES = 9 * AB_RATE;

export function startABSync(a: ABSample, b: ABSample): { promise: Promise<ABAlignment | null>; cancel(): void } {
  let worker: Worker | null = null, settled = false, resolve!: (value: ABAlignment | null) => void;
  const promise = new Promise<ABAlignment | null>(yes => { resolve = yes; });
  const finish = (value: ABAlignment | null) => {
    if (settled) return;
    settled = true;
    worker?.terminate(); worker = null;
    resolve(value);
  };
  try {
    if (typeof Worker === 'undefined') return { promise: Promise.resolve(null), cancel() {} };
    worker = new SyncWorker();
    const copy = (sample: ABSample) => {
      const frames = Math.min(sample.frames, sample.left.length, sample.right.length, ANALYSIS_FRAMES);
      return { frames, left: sample.left.slice(0, frames), right: sample.right.slice(0, frames) };
    };
    const left = copy(a), right = copy(b);
    worker.onmessage = event => {
      const result = event.data as Partial<ABAlignment> | null;
      const offsetFrames = result?.offsetFrames;
      finish(result && typeof offsetFrames === 'number' && Number.isSafeInteger(offsetFrames) && Math.abs(offsetFrames) <= 3 * AB_RATE
        && typeof result.confidence === 'number' && Number.isFinite(result.confidence) && result.confidence >= 0 && result.confidence <= 1
        && (result.windowSeconds === 3 || result.windowSeconds === 6)
        ? result as ABAlignment : null);
    };
    worker.onerror = event => { event.preventDefault(); finish(null); };
    worker.onmessageerror = () => finish(null);
    worker.postMessage({ a: left, b: right }, [left.left.buffer, left.right.buffer, right.left.buffer, right.right.buffer]);
  } catch {
    finish(null);
  }
  return { promise, cancel: () => finish(null) };
}
