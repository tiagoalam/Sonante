type Listener = () => void;
type Resolver = (confirmed: boolean) => void;

export interface DirectVolumeChoicePromptState { open: boolean }

let snapshot: DirectVolumeChoicePromptState = { open: false };
let active: Resolver | null = null;
const queued: Resolver[] = [];
const listeners = new Set<Listener>();

export function getDirectVolumeChoicePromptSnapshot(): DirectVolumeChoicePromptState { return snapshot; }
export function subscribeDirectVolumeChoicePrompt(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function publish(open: boolean) {
  snapshot = { open };
  listeners.forEach(listener => listener());
}

function activateNext() {
  if (active) return;
  active = queued.shift() ?? null;
  publish(!!active);
}

export function requestDirectVolumeChoice(): Promise<boolean> {
  return new Promise(resolve => { queued.push(resolve); activateNext(); });
}

export function respondToDirectVolumeChoice(confirmed: boolean): void {
  if (!active) return;
  const resolve = active;
  active = null;
  publish(false);
  resolve(confirmed);
  queueMicrotask(activateNext);
}
