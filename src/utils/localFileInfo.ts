// Share only an active request, never retain a result cache. A close discards
// presentation logically; it cannot interrupt a filesystem read in Rust.
export function singleFileInspection<T>(read: (path: string) => Promise<T>) {
  let pending: { path: string; context: unknown; promise: Promise<T> } | null = null;
  return (path: string, context?: unknown): Promise<T> => {
    if (pending) return pending.path === path && pending.context === context ? pending.promise : Promise.reject("busy");
    const promise = read(path).finally(() => { if (pending?.promise === promise) pending = null; });
    pending = { path, context, promise };
    return promise;
  };
}

export function fileInfoErrorKey(error: unknown): string {
  const known = ["invalid_local_path", "outside_configured_roots", "file_not_found", "cue_virtual",
    "unsupported_format", "corrupt_header", "permission_denied", "unavailable", "stale", "busy"];
  return typeof error === "string" && known.includes(error) ? error : "unavailable";
}
