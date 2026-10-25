// Each read retains the backend startup gate until I/O and final validation
// finish. Avoid competing with our own concurrent webview read requests.
// Playback, Save and cancellation never enter this queue.
export function createLocalSourceReadQueue() {
  let settled: Promise<void> = Promise.resolve();
  return function run<T>(read: () => Promise<T>): Promise<T> {
    const result = settled.then(read);
    // Only the queue tail absorbs rejection; the caller receives the exact error.
    settled = result.then(() => undefined, () => undefined);
    return result;
  };
}
