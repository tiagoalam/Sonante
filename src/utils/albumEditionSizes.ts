import type { EditionEntry, EditionGroup } from './albumEditions';
export interface EditionVariant { key: string; entries: EditionEntry[]; sizes: readonly number[] | null }
export type SizeObservations = ReadonlyMap<string, readonly number[] | null>;
export function validFileSizes(value: unknown): value is number[] {
  return Array.isArray(value) && value.length > 0 && value.length <= 500
    && value.every(n => Number.isSafeInteger(n) && n > 0);
}
export function editionSizeVariants(group: EditionGroup, observations: SizeObservations): EditionVariant[] {
  const variants = new Map<string, EditionVariant>();
  for (const entry of group.entries) {
    const observed = observations.get(entry.key);
    const sizes = validFileSizes(observed) ? [...observed].sort((a, b) => a - b) : null;
    // Unknown values are always separate, never a shared "null" identity.
    const key = sizes ? JSON.stringify(['bytes', sizes]) : JSON.stringify(['unknown', entry.key]);
    const variant = variants.get(key);
    if (variant) variant.entries.push(entry); else variants.set(key, { key, entries: [entry], sizes });
  }
  return [...variants.values()];
}
export function createEditionSizeReadQueue() {
  let active = 0;
  const pending: Array<() => void> = [];
  const drain = () => { while (active < 2 && pending.length) pending.shift()!(); };
  return <T>(work: () => Promise<T>, current: () => boolean): Promise<T> => new Promise((resolve, reject) => {
    pending.push(() => {
      if (!current()) { reject('EDITIONS_CANCELLED'); return; }
      active++;
      void Promise.resolve().then(() => { if (!current()) throw 'EDITIONS_CANCELLED'; return work(); }).then(resolve, reject)
        .finally(() => { active--; drain(); });
    });
    queueMicrotask(drain);
  });
}
