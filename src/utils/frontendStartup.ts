// Passive first-commit measurements, relative to this renderer's navigation.
// No polling, source contents, configuration, metadata, URLs or native requests.
type Stage = 'renderer_entry' | 'shell_committed' | 'config_committed';
const stages: readonly Stage[] = ['renderer_entry', 'shell_committed', 'config_committed'];
export function createFrontendStartup(now: () => number = () => performance.now()) {
 const observed = new Map<Stage, number>();
 return {
  mark(stage: Stage) {
   if (!stages.includes(stage) || observed.has(stage)) return;
   const elapsed = now();
   if (Number.isFinite(elapsed) && elapsed >= 0 && elapsed <= Number.MAX_SAFE_INTEGER) observed.set(stage, Math.round(elapsed));
  },
  snapshot() { return Object.fromEntries(observed); },
 };
}
export const frontendStartup = createFrontendStartup();
