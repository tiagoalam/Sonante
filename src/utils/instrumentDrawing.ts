import { INSTRUMENT_BANDS, meterPosition, type InstrumentFrame } from "./instrumentMotion";
export type InstrumentMode = "combo" | "spectrum-neon" | "spectrum-segmented" | "vu-amber" | "vu-blue" | "vu-radial" | "levels";
export interface InstrumentLabels { left: string; right: string; relative: string; }
const START = -Math.PI * .8, END = -Math.PI * .2;
const point = (x: number, y: number, r: number, angle: number) => [x + Math.cos(angle) * r, y + Math.sin(angle) * r] as const;
const angleFor = (value: number) => START + value * (END - START);
const isVu = (mode: InstrumentMode) => mode === "vu-amber" || mode === "vu-blue";
const meterGeometry = (width: number, height: number, channel: number) => {
  const gap = Math.max(8, width * .025), w = (width - gap) / 2, x = channel * (w + gap);
  return { x, w, cx: x + w / 2, cy: height * .85, r: Math.max(1, Math.min(w * .42, height * .62)) };
};
const rounded = (ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) => {
  ctx.beginPath(); ctx.roundRect(x, y, Math.max(0, w), Math.max(0, h), r);
};
const line = (ctx: CanvasRenderingContext2D, a: readonly number[], b: readonly number[]) => {
  ctx.beginPath(); ctx.moveTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); ctx.stroke();
};

function meterFace(ctx: CanvasRenderingContext2D, width: number, height: number, channel: number, blue: boolean, labels: InstrumentLabels) {
  const { x, w, cx, cy, r } = meterGeometry(width, height, channel), inset = 4;
  const bezel = ctx.createLinearGradient(0, 0, 0, height);
  bezel.addColorStop(0, "#72747a"); bezel.addColorStop(.035, "#1a1c20"); bezel.addColorStop(.96, "#06080b"); bezel.addColorStop(1, "#505157");
  ctx.fillStyle = bezel; rounded(ctx, x, 0, w, height, blue ? 12 : 6); ctx.fill();
  const paper = ctx.createLinearGradient(0, inset, 0, height - inset);
  paper.addColorStop(0, blue ? "#5396ad" : "#ae7b2d"); paper.addColorStop(.12, blue ? "#c2edf0" : "#f8dfa0");
  paper.addColorStop(.58, blue ? "#d9f5ef" : "#ffedbc"); paper.addColorStop(1, blue ? "#71bacd" : "#c9a65b");
  ctx.fillStyle = paper; rounded(ctx, x + inset, inset, w - inset * 2, height - inset * 2, blue ? 8 : 2); ctx.fill();
  const ink = blue ? "#153b4a" : "#4b3821";
  ctx.strokeStyle = ink; ctx.lineWidth = 1;
  ctx.beginPath(); ctx.arc(cx, cy, r, START, END); ctx.stroke();
  ctx.strokeStyle = "#b53729"; ctx.lineWidth = 2.4;
  ctx.beginPath(); ctx.arc(cx, cy, r, angleFor(42 / 48), END); ctx.stroke();
  for (let db = -48; db <= 0; db += 2) {
    const major = db % 10 === 0 || db === -6 || db === -48;
    const angle = angleFor((db + 48) / 48);
    ctx.strokeStyle = db >= -6 ? "#a92d24" : ink; ctx.lineWidth = major ? 1.2 : .65;
    line(ctx, point(cx, cy, r - (major ? 9 : 5), angle), point(cx, cy, r + 1, angle));
  }
  ctx.font = `${Math.max(7, Math.min(11, w * .041))}px ui-sans-serif,sans-serif`; ctx.textAlign = "center"; ctx.textBaseline = "middle";
  for (const db of [-40, -30, -20, -10, -6, 0]) {
    const at = point(cx, cy, r + Math.max(10, w * .046), angleFor((db + 48) / 48));
    ctx.fillStyle = db >= -6 ? "#a92d24" : ink; ctx.fillText(String(db), at[0], at[1]);
  }
  ctx.fillStyle = ink; ctx.font = `700 ${Math.max(11, Math.min(22, w * .075))}px Georgia,serif`;
  ctx.fillText("VU", cx, height * .53);
  ctx.font = `${Math.max(7, Math.min(9, w * .035))}px ui-monospace,monospace`;
  ctx.fillText("RMS · dBFS", cx, height * .63);
  ctx.font = `600 ${Math.max(7, Math.min(10, w * .035))}px ui-sans-serif,sans-serif`;
  ctx.textAlign = "left"; ctx.fillText(channel ? labels.right : labels.left, x + 12, Math.max(14, height * .095));
  ctx.textAlign = "center";
  // Glass edge/reflection belongs to the cached face, not to each animated frame.
  const glass = ctx.createLinearGradient(0, 4, 0, Math.max(5, height * .32));
  glass.addColorStop(0, "#ffffff35"); glass.addColorStop(1, "#ffffff00");
  ctx.fillStyle = glass; rounded(ctx, x + 6, 5, w - 12, Math.max(1, height * .25), 2); ctx.fill();
  ctx.strokeStyle = "#00000065"; ctx.lineWidth = 2; rounded(ctx, x + 4, 4, w - 8, height - 8, blue ? 8 : 2); ctx.stroke();
}

function meterNeedle(ctx: CanvasRenderingContext2D, width: number, height: number, channel: number, frame: InstrumentFrame, blue: boolean) {
  const { x, w, cx, cy, r } = meterGeometry(width, height, channel), angle = angleFor(meterPosition(frame.rms[channel]));
  const tip = point(cx, cy, r - 5, angle), tail = point(cx, cy, -9, angle), side = [-Math.sin(angle) * 1.1, Math.cos(angle) * 1.1];
  ctx.fillStyle = "#00000035"; ctx.beginPath(); ctx.moveTo(tail[0] + 2, tail[1] + 2); ctx.lineTo(tip[0] + 2, tip[1] + 2); ctx.lineTo(cx + side[0] + 2, cy + side[1] + 2); ctx.closePath(); ctx.fill();
  ctx.fillStyle = blue ? "#183944" : "#261c15";
  ctx.beginPath(); ctx.moveTo(tail[0] - side[0], tail[1] - side[1]); ctx.lineTo(tip[0], tip[1]); ctx.lineTo(tail[0] + side[0], tail[1] + side[1]); ctx.closePath(); ctx.fill();
  if (frame.peak[channel] > 0) {
    const peak = angleFor(meterPosition(frame.peak[channel])); ctx.strokeStyle = blue ? "#137d98" : "#ba3e24"; ctx.lineWidth = 2;
    line(ctx, point(cx, cy, r + 2, peak), point(cx, cy, r - 3, peak));
  }
  ctx.fillStyle = "#24262a"; ctx.beginPath(); ctx.arc(cx, cy, 6.5, 0, Math.PI * 2); ctx.fill();
  ctx.fillStyle = blue ? "#abcad0" : "#c9a866"; ctx.beginPath(); ctx.arc(cx - 1, cy - 1, 3.2, 0, Math.PI * 2); ctx.fill();
  ctx.fillStyle = frame.peak[channel] > .944 ? "#e44228" : "#49281e";
  ctx.beginPath(); ctx.arc(x + w - 14, 14, 2.3, 0, Math.PI * 2); ctx.fill();
}

const radialAngle = (value: number) => Math.PI * (.75 + value * 1.5);
const radialGeometry = (width: number, height: number, channel: number) => {
  const gap = Math.max(12, width * .035), slot = (width - gap) / 2;
  return { cx: slot / 2 + channel * (slot + gap), cy: height * .5, r: Math.max(1, Math.min(slot * .46, height * .465)) };
};
function radialFace(ctx: CanvasRenderingContext2D, width: number, height: number, channel: number, labels: InstrumentLabels) {
  const { cx, cy, r } = radialGeometry(width, height, channel);
  const metal = ctx.createLinearGradient(cx - r, cy - r, cx + r, cy + r);
  metal.addColorStop(0, "#7e858d"); metal.addColorStop(.23, "#20252b"); metal.addColorStop(.50, "#070a0e"); metal.addColorStop(.78, "#414b55"); metal.addColorStop(1, "#15191e");
  ctx.fillStyle = metal; ctx.beginPath(); ctx.arc(cx, cy, r, 0, Math.PI * 2); ctx.fill();
  const glass = ctx.createRadialGradient(cx - r * .18, cy - r * .36, 0, cx, cy, r * .96);
  glass.addColorStop(0, "#252d36"); glass.addColorStop(.58, "#10151c"); glass.addColorStop(1, "#04070b");
  ctx.fillStyle = glass; ctx.beginPath(); ctx.arc(cx, cy, r * .95, 0, Math.PI * 2); ctx.fill();
  ctx.strokeStyle = "#aab9c024"; ctx.lineWidth = 1;
  ctx.beginPath(); ctx.arc(cx, cy, r * .91, 0, Math.PI * 2); ctx.stroke();
  ctx.strokeStyle = "#b8c8d046"; ctx.beginPath(); ctx.arc(cx, cy, r * .83, radialAngle(0), radialAngle(1)); ctx.stroke();
  ctx.strokeStyle = "#e38e4b"; ctx.lineWidth = 2;
  ctx.beginPath(); ctx.arc(cx, cy, r * .83, radialAngle(42 / 48), radialAngle(1)); ctx.stroke();
  for (let db = -48; db <= 0; db += 2) {
    const major = db % 12 === 0 || db === -6, angle = radialAngle((db + 48) / 48);
    ctx.strokeStyle = db >= -6 ? "#f2a666" : major ? "#d7e4e8" : "#778c99"; ctx.lineWidth = major ? 1.6 : .8;
    line(ctx, point(cx, cy, r * (major ? .72 : .77), angle), point(cx, cy, r * .85, angle));
  }
  ctx.font = `${Math.max(7, Math.min(11, r * .12))}px ui-monospace,monospace`; ctx.textAlign = "center"; ctx.textBaseline = "middle";
  for (const db of [-48, -36, -24, -12, -6, 0]) {
    const at = point(cx, cy, r * .59, radialAngle((db + 48) / 48));
    ctx.fillStyle = db >= -6 ? "#eda466" : "#b6c6ce"; ctx.fillText(String(db), at[0], at[1]);
  }
  ctx.fillStyle = "#dee5e4"; ctx.font = `700 ${Math.max(10, Math.min(20, r * .20))}px ui-sans-serif,sans-serif`;
  ctx.fillText("dBFS", cx, cy + r * .39);
  ctx.fillStyle = "#81939f"; ctx.font = `${Math.max(7, Math.min(9, r * .09))}px ui-monospace,monospace`;
  ctx.fillText("RMS", cx, cy + r * .57);
  ctx.fillStyle = "#d8b17a"; ctx.font = `600 ${Math.max(7, Math.min(10, r * .10))}px ui-sans-serif,sans-serif`;
  ctx.fillText(channel ? labels.right : labels.left, cx, cy + r * .78);
}
function radialNeedle(ctx: CanvasRenderingContext2D, width: number, height: number, channel: number, frame: InstrumentFrame) {
  const { cx, cy, r } = radialGeometry(width, height, channel), angle = radialAngle(meterPosition(frame.rms[channel]));
  const tip = point(cx, cy, r * .74, angle), tail = point(cx, cy, -r * .17, angle), side = [-Math.sin(angle) * 1.5, Math.cos(angle) * 1.5];
  ctx.fillStyle = "#e9e4cd"; ctx.beginPath();
  ctx.moveTo(tail[0] - side[0], tail[1] - side[1]); ctx.lineTo(tip[0], tip[1]); ctx.lineTo(tail[0] + side[0], tail[1] + side[1]); ctx.closePath(); ctx.fill();
  if (frame.peak[channel] > 0) {
    const a = radialAngle(meterPosition(frame.peak[channel])); ctx.strokeStyle = "#e1a364"; ctx.lineWidth = 2.2;
    line(ctx, point(cx, cy, r * .89, a), point(cx, cy, r * .83, a));
  }
  ctx.fillStyle = "#06090d"; ctx.beginPath(); ctx.arc(cx, cy, Math.max(3, r * .075), 0, Math.PI * 2); ctx.fill();
  ctx.strokeStyle = "#98a8b4"; ctx.lineWidth = 1.2; ctx.stroke();
  ctx.fillStyle = "#b9c8d0"; ctx.beginPath(); ctx.arc(cx - .7, cy - .7, Math.max(1.5, r * .025), 0, Math.PI * 2); ctx.fill();
}

function spectrumArea(width: number, height: number, combo: boolean) {
  const top = combo ? 48 : 12;
  return { x: 28, y: top, w: Math.max(1, width - 34), h: Math.max(1, height - top - 20) };
}
function spectrumFace(ctx: CanvasRenderingContext2D, width: number, height: number, combo: boolean) {
  const a = spectrumArea(width, height, combo);
  ctx.font = "8px ui-monospace,monospace"; ctx.textAlign = "right"; ctx.textBaseline = "middle";
  for (const db of [0, -30, -60, -90]) {
    const y = a.y + a.h * -db / 90; ctx.strokeStyle = "#b4dce614"; ctx.lineWidth = 1;
    line(ctx, [a.x, y], [width - 6, y]); ctx.fillStyle = "#b4dce66b"; ctx.fillText(String(db), a.x - 5, y);
  }
  ctx.fillStyle = "#b4dce66b"; ctx.textBaseline = "bottom";
  const frequencies: [number, string][] = [[40,"40"],[100,"100"],[1000,"1k"],[10000,"10k"],[20000,"20k Hz"]];
  frequencies.forEach(([hz, text], index) => {
    const x = a.x + Math.log(hz / 40) / Math.log(500) * a.w;
    ctx.textAlign = index === 0 ? "left" : index === frequencies.length - 1 ? "right" : "center";
    ctx.fillText(text, x, height - 2);
  });
}
function spectrumBars(ctx: CanvasRenderingContext2D, width: number, height: number, frame: InstrumentFrame, segmented: boolean, combo: boolean) {
  const a = spectrumArea(width, height, combo), gap = Math.max(1, Math.min(3, a.w / 250));
  const bar = Math.max(.5, (a.w - gap * (INSTRUMENT_BANDS - 1)) / INSTRUMENT_BANDS);
  const gradient = ctx.createLinearGradient(0, a.y + a.h, 0, a.y);
  gradient.addColorStop(0, "#165c70"); gradient.addColorStop(.55, segmented ? "#45aeb3" : "#27c6d1"); gradient.addColorStop(.85, "#a0e6cc"); gradient.addColorStop(1, "#efd18a");
  ctx.fillStyle = gradient;
  for (let i = 0; i < INSTRUMENT_BANDS; i++) {
    const h = frame.spectrum[i] * a.h, x = a.x + i * (bar + gap);
    ctx.fillRect(x, a.y + a.h - h, bar, h);
  }
  if (segmented) {
    // Fixed horizontal gaps cut every column at once instead of 48x20 rectangles.
    ctx.strokeStyle = "#071016"; ctx.lineWidth = Math.max(1, Math.min(2, a.h / 35));
    for (let row = 1; row < 20; row++) { const y = a.y + a.h * row / 20; line(ctx, [a.x, y], [a.x + a.w, y]); }
  }
  ctx.fillStyle = "#d6f4edcc";
  for (let i = 0; i < INSTRUMENT_BANDS; i++) {
    if (frame.spectrumPeaks[i] <= 0) continue;
    ctx.fillRect(a.x + i * (bar + gap), a.y + a.h * (1 - frame.spectrumPeaks[i]), bar, 1);
  }
}
function levelBars(ctx: CanvasRenderingContext2D, width: number, height: number, frame: InstrumentFrame, only: boolean) {
  const top = only ? Math.max(12, height * .15) : 6, rowHeight = only ? Math.min(40, height * .23) : 12;
  const x = 28, w = Math.max(1, width - 34);
  for (let channel = 0; channel < 2; channel++) {
    const y = top + channel * (rowHeight + (only ? 14 : 7));
    ctx.fillStyle = "#778e9220"; ctx.fillRect(x, y, w, rowHeight);
    const gradient = ctx.createLinearGradient(x, 0, x + w, 0);
    gradient.addColorStop(0, "#3b9b8f"); gradient.addColorStop(.75, "#d2b654"); gradient.addColorStop(.95, "#d66a39"); gradient.addColorStop(1, "#dd422f");
    ctx.fillStyle = gradient; ctx.fillRect(x, y, meterPosition(frame.rms[channel], -60) * w, rowHeight);
    ctx.strokeStyle = "#071016"; ctx.lineWidth = 1;
    for (let segment = 1; segment < 40; segment++) line(ctx, [x + w * segment / 40, y], [x + w * segment / 40, y + rowHeight]);
    if (frame.peak[channel] > 0) { ctx.fillStyle = "#ebf4e4"; ctx.fillRect(x + meterPosition(frame.peak[channel], -60) * (w - 1), y, 1, rowHeight); }
    ctx.fillStyle = "#c6d4d9"; ctx.font = "9px ui-monospace,monospace"; ctx.textAlign = "left"; ctx.textBaseline = "middle"; ctx.fillText(channel ? "R" : "L", 7, y + rowHeight / 2);
  }
}

export function drawInstrumentBackground(ctx: CanvasRenderingContext2D, width: number, height: number, mode: InstrumentMode, labels: InstrumentLabels) {
  ctx.fillStyle = "#071016"; ctx.fillRect(0, 0, width, height);
  if (mode === "vu-radial") for (let i = 0; i < 2; i++) radialFace(ctx, width, height, i, labels);
  else if (isVu(mode)) for (let i = 0; i < 2; i++) meterFace(ctx, width, height, i, mode === "vu-blue", labels);
  else if (mode !== "levels") spectrumFace(ctx, width, height, mode === "combo");
}
export function drawInstrumentForeground(ctx: CanvasRenderingContext2D, width: number, height: number, mode: InstrumentMode, frame: InstrumentFrame) {
  if (mode === "vu-radial") for (let i = 0; i < 2; i++) radialNeedle(ctx, width, height, i, frame);
  else if (isVu(mode)) for (let i = 0; i < 2; i++) meterNeedle(ctx, width, height, i, frame, mode === "vu-blue");
  else {
    if (mode === "combo" || mode === "levels") levelBars(ctx, width, height, frame, mode === "levels");
    if (mode !== "levels") spectrumBars(ctx, width, height, frame, mode === "spectrum-segmented", mode === "combo");
  }
}

export function instrumentCanvasSize(width: number, height: number, pixelRatio: number) {
  const w = Number.isFinite(width) ? Math.max(1, width) : 1, h = Number.isFinite(height) ? Math.max(1, height) : 1;
  const ratio = Math.min(Number.isFinite(pixelRatio) ? Math.max(1, pixelRatio) : 1, 1.5, 1600 / w, 800 / h);
  return { width: Math.max(1, Math.round(w * ratio)), height: Math.max(1, Math.round(h * ratio)), ratio, logicalWidth: w, logicalHeight: h };
}
