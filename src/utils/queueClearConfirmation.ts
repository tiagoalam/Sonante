import type { AppConfig, ConfigSaveOperation, QueueClearAuthorization } from "../types/config.ts";

export interface QueueClearConfirmation {
  operationId: string;
  config: AppConfig;
  binding: string;
}

export function queueClearBinding(error: string | null): string | null {
  // Accept only the exact refusal, optionally with the backend's confirmed
  // rollback suffix. Never recover a binding from generic/ambiguous error text.
  const match = error?.match(/^LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED:([a-f0-9]{64})(?:\. Estado anterior restaurado\.)?$/);
  return match && match[0] === error ? match[1] : null;
}

const copyConfig = (config: AppConfig): AppConfig => ({ ...config, local_folders: [...config.local_folders] });

// Keep proposals only in memory, associated with the accepted operation. A
// renderer reload cannot safely invent the proposal of a historical refusal.
export function createQueueClearConfirmationFlow() {
  let attempt: { config: AppConfig; operationId: string | null } | null = null;
  let pending: QueueClearConfirmation | null = null;
  let latestOperationId: string | null = null;
  return {
    begin(config: AppConfig): AppConfig {
      pending = null;
      attempt = { config: copyConfig(config), operationId: null };
      return copyConfig(attempt.config);
    },
    accepted(operationId: string) {
      if (attempt) attempt.operationId = operationId;
    },
    observe(operation: ConfigSaveOperation): QueueClearConfirmation | null {
      if (latestOperationId && BigInt(operation.id) < BigInt(latestOperationId)) return null;
      latestOperationId = operation.id;
      if (pending && BigInt(operation.id) > BigInt(pending.operationId)) pending = null;
      if (attempt?.operationId && BigInt(operation.id) > BigInt(attempt.operationId)) attempt = null;
      if (!attempt || attempt.operationId !== operation.id) return null;
      if (["preparing", "removing", "indexing", "validating"].includes(operation.phase)) return null;
      const config = attempt.config;
      attempt = null;
      const binding = operation.phase === "failed" ? queueClearBinding(operation.error) : null;
      if (!binding) return null;
      pending = { operationId: operation.id, config: copyConfig(config), binding };
      return pending;
    },
    isPending: () => pending !== null,
    cancel(operationId: string): AppConfig | null {
      if (pending?.operationId !== operationId) return null;
      const config = copyConfig(pending.config);
      pending = null;
      return config;
    },
    confirm(operationId: string): { config: AppConfig; authorization: QueueClearAuthorization } | null {
      if (pending?.operationId !== operationId) return null;
      const request = {
        config: copyConfig(pending.config),
        authorization: { confirmed: true as const, binding: pending.binding },
      };
      pending = null; // Consume before another click or asynchronous invocation.
      return request;
    },
    discard() { attempt = null; pending = null; },
  };
}
