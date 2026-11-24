// Bound fallback SQLite reads when the shared Local catalog is not yet loaded.
// Only in-flight work is shared; failures and completed results are never cached.
export function createCollectionArtworkFallback(read, limit = 2) {
  const pending = new Map(), jobs = [];
  let active = 0;
  function pump() {
    while (active < limit && jobs.length) {
      const job = jobs.shift();
      if (!job.consumers.some(current => current())) { pending.delete(job.key); job.resolve(null); continue; }
      active++;
      Promise.resolve().then(() => read(job.key)).then(job.resolve, job.reject).finally(() => {
        active--; if (pending.get(job.key) === job) pending.delete(job.key); pump();
      });
    }
  }
  return (key, current) => {
    if (!current()) return Promise.resolve(null);
    let job = pending.get(key);
    if (!job) {
      let resolve, reject;
      const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
      job = { key, promise, resolve, reject, consumers: [] };
      pending.set(key, job); jobs.push(job);
    }
    job.consumers.push(current);
    pump();
    return job.promise.then(value => current() ? value : null);
  };
}
