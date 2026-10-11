import type { DoctorEvidence, DoctorFinding, DoctorUiError, LibraryDoctorReport } from "../types/libraryDoctor";
import type { LocalAlbumCatalogState } from "./localAlbumCatalog";

export function libraryDoctorAvailable(catalog: LocalAlbumCatalogState, updating: boolean, albumId: string): boolean {
  return catalog.loaded && !catalog.loading && !catalog.error && !catalog.mpdBusy
    && !catalog.saveBlocked && !updating && catalog.albums.some(album => album.id === albumId);
}

export function doctorError(cause: unknown): DoctorUiError {
  return typeof cause === "string" && ["state_unavailable", "catalog_read_failed", "album_not_found", "task_failed", "read_in_progress"].includes(cause)
    ? cause as DoctorUiError : "unknown";
}

export function doctorValue(value: DoctorEvidence["value"]): { kind: "null" | "empty" | "value"; text: string } {
  return value === null ? { kind: "null", text: "null" }
    : value === "" ? { kind: "empty", text: '""' }
    : { kind: "value", text: JSON.stringify(value) };
}

export function doctorFindingGroups(report: LibraryDoctorReport): { code: DoctorFinding["code"]; findings: DoctorFinding[] }[] {
  const groups = new Map<DoctorFinding["code"], DoctorFinding[]>();
  for (const finding of report.findings) groups.set(finding.code, [...groups.get(finding.code) ?? [], finding]);
  return Array.from(groups, ([code, findings]) => ({ code, findings }));
}

export function doctorCoverageState(report: LibraryDoctorReport): "no_reads" | "partial" | "performed" {
  const c = report.coverage;
  if (c.physical_tracks_read === 0) return "no_reads";
  return c.partial || !c.physical_inspection_complete || c.physical_track_count === 0
    || c.physical_tracks_read !== c.physical_track_count || c.failed_physical_track_count > 0
    || c.cue_virtual_count > 0 || report.failures.length > 0 ? "partial" : "performed";
}

export interface DoctorSessionState {
  albumId: string;
  catalogVersion: unknown;
  available: boolean;
  phase: "idle" | "loading" | "ready" | "error" | "stale" | "unavailable" | "closed";
  report: LibraryDoctorReport | null;
  error: DoctorUiError | null;
  inFlight: boolean;
}

// Invalidation discards replies; it does NOT cancel the Rust read. Keep the
// single-flight latch until that read settles, even across context changes.
export function createLibraryDoctorSession(
  load: (albumId: string) => Promise<LibraryDoctorReport>,
  isAvailable: () => boolean = () => true,
) {
  let generation = 0, active = true;
  let state: DoctorSessionState = { albumId: "", catalogVersion: null, available: false,
    phase: "idle", report: null, error: null, inFlight: false };
  const listeners = new Set<() => void>();
  const update = (patch: Partial<DoctorSessionState>, notify = true) => {
    state = { ...state, ...patch };
    if (notify && active) listeners.forEach(listener => listener());
  };
  const invalidate = (phase: DoctorSessionState["phase"], notify = true) => {
    generation++;
    update({ report: null, error: null, phase }, notify);
  };
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    syncContext(albumId: string, catalogVersion: unknown, available: boolean, notify = true) {
      if (albumId === state.albumId && catalogVersion === state.catalogVersion && available === state.available) return;
      const initial = state.albumId === "";
      generation++;
      update({ albumId, catalogVersion, available, report: null, error: null,
        phase: !available ? "unavailable" : initial ? "idle" : "stale" }, notify);
      // No automatic inspection on catalog/availability changes.
    },
    activate() { active = true; },
    close() { invalidate("closed"); active = false; },
    async inspect() {
      if (!active || state.inFlight) return;
      if (!state.available || !isAvailable()) { invalidate("unavailable"); return; }
      const request = ++generation, albumId = state.albumId;
      update({ phase: "loading", inFlight: true, report: null, error: null });
      const current = () => active && request === generation && state.available;
      try {
        const report = await load(albumId);
        if (!current()) return;
        if (!isAvailable()) { invalidate("unavailable"); return; }
        if (!report || report.album_id !== albumId) {
          update({ phase: "error", error: "report_mismatch" });
          return;
        }
        if (!report.coverage || !Array.isArray(report.findings) || !Array.isArray(report.failures)) {
          update({ phase: "error", error: "invalid_report" });
          return;
        }
        update({ phase: "ready", report });
      } catch (cause) {
        if (!current()) return;
        if (!isAvailable()) { invalidate("unavailable"); return; }
        update({ phase: "error", error: doctorError(cause) });
      } finally {
        update({ inFlight: false });
      }
    },
  };
}

export function installDoctorDialogFocus(root: HTMLElement, close: () => void): () => void {
  const doc = root.ownerDocument;
  const previous = doc.activeElement as HTMLElement | null;
  const previousOverflow = doc.body.style.overflow;
  doc.body.style.overflow = "hidden";
  root.focus();
  const keydown = (event: KeyboardEvent) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (event.key !== "Tab") return;
    const elements = Array.from(root.querySelectorAll<HTMLElement>(
      "button:not([disabled]), summary, [tabindex='0']",
    )).filter(element => element.getClientRects().length > 0);
    const first = elements[0], last = elements[elements.length - 1];
    if (!first) { event.preventDefault(); root.focus(); return; }
    if (!root.contains(doc.activeElement) || doc.activeElement === root
      || (event.shiftKey && doc.activeElement === first) || (!event.shiftKey && doc.activeElement === last)) {
      event.preventDefault(); (event.shiftKey ? last : first).focus();
    }
  };
  const focusin = (event: FocusEvent) => { if (!root.contains(event.target as Node)) root.focus(); };
  doc.addEventListener("keydown", keydown);
  doc.addEventListener("focusin", focusin);
  return () => {
    doc.removeEventListener("keydown", keydown);
    doc.removeEventListener("focusin", focusin);
    doc.body.style.overflow = previousOverflow;
    if (previous?.isConnected) previous.focus();
  };
}
