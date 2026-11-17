import { estimateABSync, type ABAlignmentInput } from '../utils/albumABSync';

interface Request { a: ABAlignmentInput; b: ABAlignmentInput }
const workerScope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<Request>) => void) | null;
  postMessage(value: unknown): void;
};

workerScope.onmessage = event => workerScope.postMessage(estimateABSync(event.data.a, event.data.b));
