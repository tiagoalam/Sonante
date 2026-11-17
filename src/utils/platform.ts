export function isWindowsPreview(): boolean {
  return typeof document !== "undefined" && document.documentElement?.dataset.sonantePlatform === "windows";
}
