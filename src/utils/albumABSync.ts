import { AB_RATE, type ABSample } from './albumAB';

const HOP = AB_RATE / 100;
const MAX_LAG = 300; // ±3 s at 10 ms resolution.
const INITIAL_WINDOW = 300;
const EXPANDED_WINDOW = 600;
const MIN_SCORE = 0.62;
const MIN_MARGIN = 0.10;
const PEAK_EXCLUSION = 15; // Ignore the same peak's immediate shoulders (150 ms).

export interface ABAlignment { offsetFrames: number; confidence: number; windowSeconds: 3 | 6 }
export type ABAlignmentInput = Pick<ABSample, 'left' | 'right' | 'frames'>;

type Features = [Float32Array, Float32Array, Float32Array, Float32Array];

function features(sample: ABAlignmentInput): Features | null {
  if (!Number.isSafeInteger(sample.frames) || sample.frames < HOP * (INITIAL_WINDOW + MAX_LAG)
    || !(sample.left instanceof Float32Array) || !(sample.right instanceof Float32Array)) return null;
  const frames = Math.min(sample.frames, Math.floor(sample.left.length), Math.floor(sample.right.length), HOP * (EXPANDED_WINDOW + MAX_LAG));
  const count = Math.floor(frames / HOP);
  if (count < INITIAL_WINDOW + MAX_LAG || sample.left.length < frames || sample.right.length < frames) return null;
  const result: Features = [new Float32Array(count), new Float32Array(count), new Float32Array(count), new Float32Array(count)];
  const alpha = 1 - Math.exp(-2 * Math.PI * 1_200 / AB_RATE);
  let lowLeft = 0, lowRight = 0, previousLog = 0;
  for (let frame = 0; frame < count; frame++) {
    let total = 0, low = 0, high = 0;
    const begin = frame * HOP, end = begin + HOP;
    for (let i = begin; i < end; i++) {
      const left = sample.left[i], right = sample.right[i];
      if (!Number.isFinite(left) || !Number.isFinite(right) || Math.abs(left) > 8 || Math.abs(right) > 8) return null;
      total += left * left + right * right;
      lowLeft += alpha * (left - lowLeft);
      lowRight += alpha * (right - lowRight);
      low += lowLeft * lowLeft + lowRight * lowRight;
      const highLeft = left - lowLeft, highRight = right - lowRight;
      high += highLeft * highLeft + highRight * highRight;
    }
    const logTotal = Math.log(total / (2 * HOP) + 1e-12);
    result[0][frame] = logTotal;
    result[1][frame] = Math.log(low / (2 * HOP) + 1e-12);
    result[2][frame] = Math.log(high / (2 * HOP) + 1e-12);
    result[3][frame] = Math.max(0, logTotal - previousLog);
    previousLog = logTotal;
  }
  return result;
}

function correlation(a: Float32Array, aStart: number, b: Float32Array, bStart: number, length: number): number | null {
  let sumA = 0, sumB = 0, squareA = 0, squareB = 0, product = 0;
  for (let i = 0; i < length; i++) {
    const x = a[aStart + i], y = b[bStart + i];
    sumA += x; sumB += y; squareA += x * x; squareB += y * y; product += x * y;
  }
  const aa = squareA - sumA * sumA / length, bb = squareB - sumB * sumB / length;
  if (aa < 1e-6 || bb < 1e-6) return null;
  return (product - sumA * sumB / length) / Math.sqrt(aa * bb);
}

function bestForWindow(a: Features, b: Features, length: number): ABAlignment | null {
  let activeBands = 0;
  for (let band = 0; band < 4; band++) {
    if (correlation(a[band], 0, a[band], 0, length) !== null) activeBands++;
  }
  // A stable broadband envelope plus another independent band/transient feature
  // are required; flat tones and near-silence therefore stay manual.
  if (activeBands < 2) return null;

  const scores: { lag: number; score: number }[] = [];
  const weights = [0.48, 0.20, 0.20, 0.12];
  for (let lag = -MAX_LAG; lag <= MAX_LAG; lag++) {
    const aStart = lag < 0 ? -lag : 0;
    const bStart = lag > 0 ? lag : 0;
    let score = 0, weight = 0;
    for (let band = 0; band < 4; band++) {
      const value = correlation(a[band], aStart, b[band], bStart, length);
      if (value !== null) { score += value * weights[band]; weight += weights[band]; }
    }
    if (weight > 0) scores.push({ lag, score: score / weight });
  }
  scores.sort((x, y) => y.score - x.score);
  const best = scores[0];
  if (!best) return null;
  const runnerUp = scores.find(candidate => Math.abs(candidate.lag - best.lag) > PEAK_EXCLUSION);
  const margin = runnerUp ? best.score - runnerUp.score : 0;
  if (best.score < MIN_SCORE || margin < MIN_MARGIN) return null;
  return { offsetFrames: best.lag * HOP, confidence: Math.max(0, Math.min(1, best.score)), windowSeconds: (length / 100) as 3 | 6 };
}

/**
 * Finds a conservative relative start offset from short PCM envelopes. It never
 * trims by loudness: it only returns an offset when the cross-edition match is
 * strong and unambiguous. Input buffers must include up to 3 s of tail context.
 */
export function estimateABSync(a: ABAlignmentInput, b: ABAlignmentInput): ABAlignment | null {
  const af = features(a), bf = features(b);
  if (!af || !bf) return null;
  const initial = bestForWindow(af, bf, INITIAL_WINDOW);
  if (initial) return initial;
  return bestForWindow(af, bf, EXPANDED_WINDOW);
}
