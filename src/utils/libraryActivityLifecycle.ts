export const LIBRARY_ACTIVITY_SUCCESS_HIDE_MS = 60_000;

type ActivitySnapshot = { operationId: string; phase: string };

export function createLibraryActivityVisibility(
  publishHiddenOperation: (operationId: string | null) => void,
  schedule: typeof setTimeout = setTimeout,
  cancel: typeof clearTimeout = clearTimeout,
) {
  let operationId: string | null = null;
  let phase: string | null = null;
  let hiddenOperationId: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clearTimer = () => {
    if (timer === null) return;
    cancel(timer);
    timer = null;
  };
  const hide = (id: string) => {
    if (operationId !== id || hiddenOperationId === id) return;
    clearTimer();
    hiddenOperationId = id;
    publishHiddenOperation(id);
  };
  const begin = (id: string) => {
    if (!id || operationId === id) return;
    clearTimer();
    operationId = id;
    phase = null;
    hiddenOperationId = null;
    publishHiddenOperation(null);
  };

  return {
    begin,
    update(activity: ActivitySnapshot | null) {
      if (!activity) return;
      if (operationId === null) begin(activity.operationId);
      // Once a newer operation is admitted, delayed events from older work
      // cannot replace its visibility state or restart a completion timer.
      if (operationId !== activity.operationId) return;
      const enteringCompleted = activity.phase === "completed" && phase !== "completed";
      phase = activity.phase;
      if (activity.phase !== "completed") {
        clearTimer();
        return;
      }
      if (enteringCompleted && hiddenOperationId !== activity.operationId) {
        clearTimer();
        const id = activity.operationId;
        timer = schedule(() => {
          timer = null;
          if (operationId === id && phase === "completed") hide(id);
        }, LIBRARY_ACTIVITY_SUCCESS_HIDE_MS);
      }
    },
    dismiss(id: string) {
      hide(id);
    },
    isHidden(id: string | null | undefined) {
      return !!id && hiddenOperationId === id;
    },
    dispose() {
      clearTimer();
      // React StrictMode replays mount effects in development. Let the replay
      // re-arm the same completion deadline without resetting dismissal state.
      phase = null;
    },
  };
}
