export function sqliteCatalogEnabledFor(isBrowser: boolean, isDevelopment: boolean, search: string): boolean {
  return isBrowser && !(isDevelopment && new URLSearchParams(search).get("local-catalog") === "mpd");
}

export function isSqliteCatalogEnabled(): boolean {
  return sqliteCatalogEnabledFor(
    typeof window !== "undefined",
    import.meta.env?.DEV === true,
    typeof window === "undefined" ? "" : window.location?.search ?? "",
  );
}
