// Source reads retain the backend's exclusive config/profile guards. A user
// gesture waits for the current read, then runs before queued gallery work.
// Native admission and cancellation remain authoritative; never retry a refusal.
export function createLocalSourceReadQueue() {
  type Job = () => Promise<void>;
  const reads: Job[] = [], controls: Job[] = [];
  let active = false;
  function drain() {
    if (active) return;
    const job = controls.shift() ?? reads.shift();
    if (!job) return;
    active = true;
    void job().then(() => { active = false; drain(); });
  }
  function enqueue<T>(queue: Job[], work: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      // Resolve/reject only this caller; a failed boundary releases the queue.
      queue.push(() => Promise.resolve().then(work).then(resolve, reject));
      queueMicrotask(drain);
    });
  }
  const run = <T>(read: () => Promise<T>): Promise<T> => enqueue(reads, read);
  return Object.assign(run, {
    control: <T>(action: () => Promise<T>): Promise<T> => enqueue(controls, action),
  });
}

// Shared by audio and Settings in this renderer. Other windows/native clients
// still use the unchanged backend guards; this queue grants no native access.
export const localSourceReadQueue = createLocalSourceReadQueue();
