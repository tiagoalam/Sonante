export type OpraLoadError =
  | "invalidQuery"
  | "timeout"
  | "networkUnavailable"
  | "catalogUnavailable"
  | "redirectRejected"
  | "incompleteCatalog"
  | "invalidJsonl"
  | "unsupportedSchema"
  | "incompatibleSchema"
  | "invalidRecord"
  | "invalidCatalog"
  | "emptyCatalog"
  | "catalogTooLarge"
  | "unsafeCache"
  | "cacheUnavailable"
  | "unknown";

const codes: Record<string, OpraLoadError> = {
  OPRA_QUERY_INVALID: "invalidQuery",
  OPRA_NETWORK_TIMEOUT: "timeout",
  OPRA_NETWORK_UNAVAILABLE: "networkUnavailable",
  OPRA_CATALOG_UNAVAILABLE: "catalogUnavailable",
  OPRA_REDIRECT_REJECTED: "redirectRejected",
  OPRA_CATALOG_INCOMPLETE: "incompleteCatalog",
  OPRA_CATALOG_INVALID_JSONL: "invalidJsonl",
  OPRA_CATALOG_SCHEMA_UNSUPPORTED: "unsupportedSchema",
  OPRA_CATALOG_SCHEMA_INCOMPATIBLE: "incompatibleSchema",
  OPRA_CATALOG_RECORD_INVALID: "invalidRecord",
  OPRA_CATALOG_INVALID: "invalidCatalog",
  OPRA_CATALOG_EMPTY: "emptyCatalog",
  OPRA_CATALOG_TOO_LARGE: "catalogTooLarge",
  OPRA_CACHE_UNSAFE: "unsafeCache",
  OPRA_CACHE_UNAVAILABLE: "cacheUnavailable",
};

export function opraLoadError(reason: unknown): OpraLoadError {
  const message = typeof reason === "string" ? reason : reason instanceof Error ? reason.message : "";
  const match = /\b(OPRA_[A-Z_]+)\b/.exec(message);
  return match ? codes[match[1]] ?? "unknown" : "unknown";
}
