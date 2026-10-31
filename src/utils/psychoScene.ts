import type { AudioLevelFrame } from "../types/audio";
const unit = (value: number) => Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;
const TAU = Math.PI * 2;
export const PSYCHO_SCENES = ["aurora", "mandala", "cosmos"] as const;
export type PsychoScene = typeof PSYCHO_SCENES[number];
export type PsychoQuality = "auto" | "economy";
export type PsychoDetail = "rich" | "light";
export interface PsychoPreferences { scene: PsychoScene; quality: PsychoQuality; }
export const PSYCHO_PREFERENCES_KEY = "sonante.nowPlaying.psycho.v2";
export const psychoSceneName = (value: unknown): PsychoScene => PSYCHO_SCENES.find(scene => scene === value) ?? "aurora";
export function parsePsychoPreferences(value: string | null): PsychoPreferences {
  try {
    const data: unknown = value ? JSON.parse(value) : null;
    if (data && typeof data === "object") {
      const p = data as Partial<PsychoPreferences>;
      return { scene: psychoSceneName(p.scene), quality: p.quality === "economy" ? "economy" : "auto" };
    }
  } catch { /* A visual preference never blocks playback. */ }
  return { scene: "aurora", quality: "auto" };
}
export function readPsychoPreferences(): PsychoPreferences {
  try { return parsePsychoPreferences(window.localStorage.getItem(PSYCHO_PREFERENCES_KEY)); }
  catch { return parsePsychoPreferences(null); }
}
export function savePsychoPreferences(value: PsychoPreferences): void {
  try { window.localStorage.setItem(PSYCHO_PREFERENCES_KEY, JSON.stringify(value)); }
  catch { /* The preference remains available in this session. */ }
}

export function psychoEnergy(frame: AudioLevelFrame) {
  let bass = 0;
  const count = Math.min(8, frame.spectrum.length);
  for (let i = 0; i < count; i++) bass += unit(frame.spectrum[i]);
  return { level: (unit(frame.leftRms) + unit(frame.rightRms)) / 2,
    bass: count ? bass / count : 0, peak: Math.max(unit(frame.leftPeak), unit(frame.rightPeak)) };
}

export function psychoCanvasSize(width: number, height: number, pixelRatio: number, detail: PsychoDetail = "rich") {
  const w = Number.isFinite(width) ? Math.max(1, width) : 1;
  const h = Number.isFinite(height) ? Math.max(1, height) : 1;
  const scale = Math.min(Number.isFinite(pixelRatio) ? Math.max(1, pixelRatio) : 1, 1.5,
    (detail === "light" ? 896 : 1280) / w, (detail === "light" ? 560 : 800) / h);
  return { width: Math.max(1, Math.round(w * scale)), height: Math.max(1, Math.round(h * scale)) };
}

// Reduce once per mounted session under sustained pressure; no probing/recovery loop.
export class PsychoRenderBudget {
  detail: PsychoDetail;
  private samples = 0;
  private cost = 0;
  private slowFrames = 0;
  private expensiveFrames = 0;
  constructor(quality: PsychoQuality = "auto") { this.detail = quality === "economy" ? "light" : "rich"; }
  get interval() { return 1000 / (this.detail === "light" ? 24 : 30); }
  observe(renderMs: number, frameGap: number): boolean {
    if (this.detail === "light") return false;
    const cost = Number.isFinite(renderMs) ? Math.max(0, Math.min(100, renderMs)) : 100;
    this.cost += (cost - this.cost) * .12;
    this.samples++;
    this.expensiveFrames = cost > 10 ? this.expensiveFrames + 1 : Math.max(0, this.expensiveFrames - 1);
    this.slowFrames = frameGap > 70 ? this.slowFrames + 1 : Math.max(0, this.slowFrames - 1);
    if ((this.samples >= 24 && this.cost > 10 && this.expensiveFrames >= 6) || this.slowFrames >= 12) { this.detail = "light"; return true; }
    return false;
  }
}

type Energy = ReturnType<typeof psychoEnergy>;
const color = (hue: number, light: number, alpha: number) => `hsla(${((hue % 360) + 360) % 360},85%,${light}%,${alpha})`;

function backdrop(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, e: Energy, scene: PsychoScene) {
  ctx.globalCompositeOperation = "source-over";
  ctx.fillStyle = "#060817"; ctx.fillRect(0, 0, w, h);
  const halo = ctx.createRadialGradient(w * .5, h * .48, 0, w * .5, h * .48, Math.max(w, h) * .7);
  const hue = scene === "aurora" ? 165 : scene === "mandala" ? 255 : 215;
  halo.addColorStop(0, color(hue + Math.sin(t * .08) * 35, 16 + e.bass * 9, .95));
  halo.addColorStop(.48, "#10112a"); halo.addColorStop(1, "#060817");
  ctx.fillStyle = halo; ctx.fillRect(0, 0, w, h);
}

function stars(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, e: Energy, detail: PsychoDetail, travel = false) {
  const count = detail === "light" ? 28 : 64;
  for (let i = 0; i < count; i++) {
    const seed = ((i * 0.61803398875 + .13) % 1);
    const depth = travel ? ((seed + t * .018) % 1) : seed;
    const angle = i * 2.39996323 + t * .018;
    const reach = travel ? depth * depth * .8 : (.08 + seed * .65);
    const x = w * .5 + Math.cos(angle) * reach * w, y = h * .48 + Math.sin(angle) * reach * h;
    const radius = .6 + depth * (travel ? 2.3 : 1.1) + e.peak * .5;
    ctx.fillStyle = color(185 + i * 7, 78, .12 + depth * .46);
    ctx.beginPath(); ctx.arc(x, y, radius, 0, TAU); ctx.fill();
  }
}

function aurora(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, e: Energy, detail: PsychoDetail) {
  const size = Math.min(w, h), layers = detail === "light" ? 4 : 6, points = detail === "light" ? 64 : 96;
  for (let layer = 0; layer < layers; layer++) {
    const hue = 170 + layer * 37 + t * 6;
    const curve = (u: number) => h * .48 + Math.sin(u * TAU * (1.03 + layer * .08) + t * .19 + layer * .65) * size * (.15 + e.bass * .13)
      + Math.sin(u * TAU * 2.3 - t * .28 + layer) * size * (.025 + e.level * .04) + (layer - layers / 2) * size * .045;
    const thickness = size * (.035 + e.level * .055);
    const fill = ctx.createLinearGradient(0, h * .15, w, h * .85);
    fill.addColorStop(0, color(hue, 48, .02)); fill.addColorStop(.45, color(hue + 35, 60, .18 + e.level * .16)); fill.addColorStop(1, color(hue + 65, 52, .02));
    ctx.beginPath();
    for (let i = 0; i <= points; i++) { const u = i / points, x = u * w, y = curve(u); if (!i) ctx.moveTo(x, y); else ctx.lineTo(x, y); }
    for (let i = points; i >= 0; i--) { const u = i / points; ctx.lineTo(u * w, curve(u) + Math.sin(u * Math.PI) * thickness); }
    ctx.closePath(); ctx.fillStyle = fill; ctx.fill();
    ctx.beginPath();
    for (let i = 0; i <= points; i++) { const u = i / points; if (!i) ctx.moveTo(0, curve(0)); else ctx.lineTo(u * w, curve(u)); }
    ctx.strokeStyle = color(hue + 20, 72, .32 + e.level * .33); ctx.lineWidth = Math.max(1, size * .0025); ctx.stroke();
  }
}

function mandala(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, e: Energy, detail: PsychoDetail) {
  const size = Math.min(w, h), layers = detail === "light" ? 5 : 8, points = detail === "light" ? 96 : 144;
  ctx.save(); ctx.translate(w * .5, h * .48);
  for (let layer = layers - 1; layer >= 0; layer--) {
    const rotation = t * (layer % 2 ? .035 : -.025) + layer * .12;
    const radius = size * (.10 + layer * .039 + e.bass * .035);
    const hue = 265 + layer * 25 + t * 5;
    ctx.beginPath();
    for (let i = 0; i <= points; i++) {
      const a = i / points * TAU;
      const petal = 1 + Math.sin(a * 6 + t * .16 + layer * .4) * (.18 + e.level * .18) + Math.cos(a * 3 - t * .11) * .07;
      const r = radius * petal;
      const x = Math.cos(a + rotation) * r, y = Math.sin(a + rotation) * r;
      if (!i) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.closePath(); ctx.fillStyle = color(hue, 42, .035); ctx.fill();
    ctx.strokeStyle = color(hue, 60, .045); ctx.lineWidth = Math.max(2, size * .012); ctx.stroke();
    ctx.strokeStyle = color(hue, 72, .34 + e.level * .33); ctx.lineWidth = Math.max(1, size * .002); ctx.stroke();
  }
  // Central halo, never a flash/beat trigger.
  const center = ctx.createRadialGradient(0, 0, 0, 0, 0, size * .12);
  center.addColorStop(0, color(205 + t * 5, 80, .3 + e.level * .2)); center.addColorStop(1, "#00000000");
  ctx.fillStyle = center; ctx.fillRect(-size * .12, -size * .12, size * .24, size * .24);
  ctx.restore();
}

function cosmos(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, e: Energy, detail: PsychoDetail) {
  const size = Math.min(w, h), rings = detail === "light" ? 9 : 15, points = detail === "light" ? 64 : 96;
  ctx.save(); ctx.translate(w * .5, h * .48);
  for (let ring = 0; ring < rings; ring++) {
    const z = (ring / rings + t * .028) % 1;
    const radius = size * (.025 + z * z * .72);
    const angle = t * .045 + ring * .12;
    ctx.beginPath();
    for (let i = 0; i <= points; i++) {
      const a = i / points * TAU;
      const r = radius * (1 + Math.sin(a * 4 + t * .3 + z * 5) * (.07 + e.bass * .12));
      const x = Math.cos(a + angle) * r * 1.3, y = Math.sin(a + angle) * r;
      if (!i) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.closePath(); ctx.strokeStyle = color(185 + z * 165 + t * 5, 66 + z * 13, Math.sin(z * Math.PI) * (.4 + e.level * .35));
    ctx.lineWidth = Math.max(1, z * size * .004); ctx.stroke();
  }
  ctx.restore();
}

export function drawPsychoScene(canvas: HTMLCanvasElement, time: number, energy: Energy,
  scene: PsychoScene = "aurora", detail: PsychoDetail = "rich") {
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!ctx) return;
  const w = canvas.width, h = canvas.height, t = Number.isFinite(time) ? Math.max(0, time) : 0;
  const e = { level: unit(energy.level), bass: unit(energy.bass), peak: unit(energy.peak) };
  backdrop(ctx, w, h, t, e, scene);
  ctx.globalCompositeOperation = "screen";
  stars(ctx, w, h, t, e, detail, scene === "cosmos");
  if (scene === "mandala") mandala(ctx, w, h, t, e, detail);
  else if (scene === "cosmos") cosmos(ctx, w, h, t, e, detail);
  else aurora(ctx, w, h, t, e, detail);
  ctx.globalCompositeOperation = "source-over";
}
