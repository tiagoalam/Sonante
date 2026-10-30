import type { ConfigSaveOperation } from "../types/config.ts";

export const configSavePending = (operation: ConfigSaveOperation): boolean =>
  ["preparing", "removing", "indexing", "validating"].includes(operation.phase);

// One observer belongs to App, not the modal or the invoke promise. It survives
// modal close and reattaches to a running backend operation after renderer reload.
export function monitorConfigSave(
  read: () => Promise<ConfigSaveOperation | null>,
  onChange: (operation: ConfigSaveOperation) => void,
  onReadError: (error: unknown) => void,
  schedule: (callback: () => void) => ReturnType<typeof setTimeout> = (callback) => setTimeout(callback, 1000),
  cancel: (timer: ReturnType<typeof setTimeout>) => void = clearTimeout,
) {
  let disposed = false;
  let accepting = false;
  let acceptanceFailed = false;
  let latest: ConfigSaveOperation | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const publish = (operation: ConfigSaveOperation | null) => {
    if (disposed || accepting || !operation) return;
    // A historical completed/failed record is not the outcome of a new invoke
    // that failed admission. Only conclude an operation this observer followed.
    if (acceptanceFailed && !configSavePending(operation) && operation.phase !== "uncertain" &&
      (!latest || operation.id !== latest.id)) return;
    if (latest) {
      if (BigInt(operation.id) < BigInt(latest.id)) return;
      const order = ["preparing", "removing", "indexing", "validating", "completed", "failed", "uncertain"];
      if (operation.id === latest.id && (!configSavePending(latest) ||
        order.indexOf(operation.phase) < order.indexOf(latest.phase))) return;
    }
    latest = operation;
    onChange(operation);
  };
  const poll = async () => {
    try { publish(await read()); }
    catch (error) { if (!disposed) onReadError(error); }
    if (!disposed) timer = schedule(() => { void poll(); });
  };
  void poll();
  return {
    isPending: () => accepting || (latest !== null && configSavePending(latest)),
    beginAcceptance: () => { accepting = true; acceptanceFailed = false; },
    accepted: (operation: ConfigSaveOperation) => { accepting = false; acceptanceFailed = false; publish(operation); },
    acceptanceFailed: () => { accepting = false; acceptanceFailed = true; },
    dispose: () => {
      disposed = true;
      if (timer !== undefined) cancel(timer);
    },
  };
}
