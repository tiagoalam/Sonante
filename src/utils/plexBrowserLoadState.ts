export type PlexBrowserLoadState = "libraries_loading" | "libraries_error" | "no_libraries" | "catalog_loading" | "catalog_error";

// Never present a previous account/library failure as the current request's result.
export function plexBrowserLoadState(input: {
  loadingLibraries: boolean;
  librariesError: boolean;
  librariesIdentity: unknown;
  connectionIdentity: unknown;
  selectedLibrary: { key: string } | null;
  browserIdentity: unknown;
  albumLoad: { identity: unknown; phase: "loading" | "ready" | "error" } | null;
}): PlexBrowserLoadState | null {
  if (input.loadingLibraries || input.librariesIdentity !== input.connectionIdentity) return "libraries_loading";
  if (input.librariesError) return "libraries_error";
  if (!input.selectedLibrary) return "no_libraries";
  if (!input.albumLoad || input.albumLoad.identity !== input.browserIdentity || input.albumLoad.phase === "loading") return "catalog_loading";
  return input.albumLoad.phase === "error" ? "catalog_error" : null;
}
