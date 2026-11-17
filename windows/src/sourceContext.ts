// Native status already observed by the shared app is the only publisher.
// Logical restored queues do not authorize header reads until explicit Play.
export type SourceContext = Readonly<{generation: number; live: boolean}>;
let snapshot: SourceContext = Object.freeze({generation: 0, live: false});
const listeners = new Set<() => void>();
export const getSourceContext = () => snapshot;
export const subscribeSourceContext = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};
export function publishSourceContext(generation: number, live: boolean) {
  if (snapshot.generation === generation && snapshot.live === live) return;
  snapshot = Object.freeze({generation, live});
  for (const listener of listeners) listener();
}
