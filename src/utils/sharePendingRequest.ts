// Share only executing work. Neither success nor failure is cached.
// A hung request stays shared; a visual timeout must not release it.
export function sharePendingRequest<T>(load: () => Promise<T>): () => Promise<T> {
  let pending: Promise<T> | null = null;
  return () => {
    if (!pending) {
      pending = Promise.resolve().then(load).finally(() => { pending = null; });
    }
    return pending;
  };
}
