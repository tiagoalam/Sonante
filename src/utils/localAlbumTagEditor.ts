import type {
  LocalAlbumFieldState,
  LocalAlbumFieldSummary,
  LocalAlbumTagBatchPlan,
  LocalAlbumTagBatchPreflightRequest,
  LocalAlbumTagBatchUpdate,
  LocalAlbumTagInspection,
  TagFieldUpdate,
} from "../types/localTags";

export const localAlbumTagFields = ["album", "album_artist", "date", "genre"] as const;
export type LocalAlbumTagField = typeof localAlbumTagFields[number];

export interface LocalAlbumTagFieldDraft {
  summary: LocalAlbumFieldSummary;
  value: string;
  touched: boolean;
  clearAll: boolean;
}

export interface LocalAlbumTagEditorDraft {
  inspection: LocalAlbumTagInspection;
  fields: Record<LocalAlbumTagField, LocalAlbumTagFieldDraft>;
}

export interface LocalAlbumTagFormAnalysis {
  update: LocalAlbumTagBatchUpdate;
  dirty: boolean;
  blocked: boolean;
  blockedReason: "cue" | "inspection" | "format" | null;
}

export interface LocalAlbumTagPreviewView {
  applicable: boolean;
  blockedReason: LocalAlbumTagBatchPlan["blocked_reason"];
  changeCount: number;
  unchangedCount: number;
  canApply: false;
  files: {
    uri: string;
    needsWrite: boolean;
    blocked: boolean;
    changes: { field: LocalAlbumTagField; before: string | null; proposed: string | null }[];
  }[];
}

function initialField(summary: LocalAlbumFieldSummary): LocalAlbumTagFieldDraft {
  return {
    summary,
    value: summary.state === "common" ? summary.value ?? "" : "",
    touched: false,
    clearAll: false,
  };
}

export function createLocalAlbumTagEditorDraft(
  inspection: LocalAlbumTagInspection,
): LocalAlbumTagEditorDraft {
  return {
    inspection,
    fields: {
      album: initialField(inspection.fields.album),
      album_artist: initialField(inspection.fields.album_artist),
      date: initialField(inspection.fields.date),
      genre: initialField(inspection.fields.genre),
    },
  };
}

export function editLocalAlbumTagField(
  draft: LocalAlbumTagEditorDraft,
  field: LocalAlbumTagField,
  value: string,
): LocalAlbumTagEditorDraft {
  return {
    ...draft,
    fields: {
      ...draft.fields,
      [field]: { ...draft.fields[field], value, touched: true, clearAll: false },
    },
  };
}

export function clearMixedLocalAlbumTagField(
  draft: LocalAlbumTagEditorDraft,
  field: LocalAlbumTagField,
): LocalAlbumTagEditorDraft {
  return {
    ...draft,
    fields: {
      ...draft.fields,
      [field]: { ...draft.fields[field], value: "", touched: true, clearAll: true },
    },
  };
}

export function resetLocalAlbumTagField(
  draft: LocalAlbumTagEditorDraft,
  field: LocalAlbumTagField,
): LocalAlbumTagEditorDraft {
  return {
    ...draft,
    fields: { ...draft.fields, [field]: initialField(draft.fields[field].summary) },
  };
}

function fieldUpdate(field: LocalAlbumTagFieldDraft): TagFieldUpdate<string> {
  if (field.summary.state === "indeterminate" || field.summary.state === "not_applicable") {
    return { action: "unchanged" };
  }
  if (field.clearAll) return { action: "remove" };
  if (!field.touched) return { action: "unchanged" };

  if (field.summary.state === "common") {
    if (field.value === (field.summary.value ?? "")) return { action: "unchanged" };
    return field.value === ""
      ? { action: "remove" }
      : { action: "set", value: field.value };
  }

  if (field.value === "") return { action: "unchanged" };
  return { action: "set", value: field.value };
}

function blocksEditing(state: LocalAlbumFieldState): boolean {
  return state === "indeterminate" || state === "not_applicable";
}

export function analyzeLocalAlbumTagForm(
  draft: LocalAlbumTagEditorDraft,
): LocalAlbumTagFormAnalysis {
  const update = Object.fromEntries(
    localAlbumTagFields.map((field) => [field, fieldUpdate(draft.fields[field])]),
  ) as unknown as LocalAlbumTagBatchUpdate;
  const inspection = draft.inspection;
  const blockedReason = inspection.cue_virtual_count > 0
    || inspection.tracks.some((track) => track.kind === "cue_virtual") ? "cue"
    : !inspection.inspection_complete || inspection.physical_track_count === 0
      || inspection.tracks.some((track) => track.issue || !track.tags)
      || localAlbumTagFields.some((field) => blocksEditing(draft.fields[field].summary.state)) ? "inspection"
    : inspection.tracks.some((track) => !track.tags?.readable || !track.tags.writable || !track.tags.editable) ? "format"
    : null;
  return {
    update,
    dirty: localAlbumTagFields.some((field) => update[field].action !== "unchanged"),
    blocked: blockedReason !== null,
    blockedReason,
  };
}

export function buildLocalAlbumTagBatchPreflightRequest(
  draft: LocalAlbumTagEditorDraft,
): LocalAlbumTagBatchPreflightRequest {
  const analysis = analyzeLocalAlbumTagForm(draft);
  return {
    album_id: draft.inspection.album_id,
    tracks: draft.inspection.tracks.map((track) => ({
      uri: track.uri,
      kind: track.kind,
      revision: track.kind === "physical" ? track.tags?.revision ?? null : null,
    })),
    update: analysis.update,
  };
}

export function localAlbumTagPreviewView(
  plan: LocalAlbumTagBatchPlan,
  inspection: LocalAlbumTagInspection,
): LocalAlbumTagPreviewView {
  if (plan.album_id !== inspection.album_id) throw new Error("inspection_mismatch");
  const snapshot = new Map(inspection.tracks.filter((track) => track.kind === "physical")
    .map((track) => [track.uri, track]));
  const seen = new Set<string>();
  const files = plan.tracks.map((track) => {
    const original = snapshot.get(track.uri);
    if (seen.has(track.uri) || !original?.tags || original.tags.revision !== track.expected_revision
      || Object.entries(track.update).some(([field, update]) =>
        !localAlbumTagFields.includes(field as LocalAlbumTagField) && update.action !== "unchanged")) {
      throw new Error("inspection_mismatch");
    }
    seen.add(track.uri);
    const changes = localAlbumTagFields.flatMap((field) => {
      const update = track.update[field];
      return update.action === "unchanged" ? [] : [{
        field, before: original.tags![field],
        proposed: update.action === "remove" ? null : update.value,
      }];
    });
    if (track.needs_write !== (changes.length > 0)) throw new Error("invalid_plan");
    return { uri: track.uri, needsWrite: track.needs_write, blocked: track.issue !== null, changes };
  });
  if (seen.size !== snapshot.size
    || plan.change_count !== files.filter((file) => file.needsWrite).length
    || plan.unchanged_count !== plan.tracks.filter((track) => !track.needs_write && !track.issue).length) {
    throw new Error("invalid_plan");
  }
  return {
    applicable: plan.applicable,
    blockedReason: plan.blocked_reason,
    changeCount: plan.change_count,
    unchangedCount: plan.unchanged_count,
    canApply: false,
    files,
  };
}

// Context changes invalidate both the inspection and all pending responses.
// Editing/reloading invalidates requests without discarding the inspection.
export function createLocalAlbumTagRequestGuard() {
  let album = "", catalog: unknown, available = false, context = 0, generation = 0;
  return {
    syncContext(nextAlbum: string, nextCatalog: unknown, nextAvailable: boolean) {
      if (album !== nextAlbum || catalog !== nextCatalog || available !== nextAvailable) {
        album = nextAlbum; catalog = nextCatalog; available = nextAvailable;
        context++; generation++;
      }
      return context;
    },
    invalidate() { generation++; },
    capture() { return { context, generation: ++generation }; },
    isCurrent(token: { context: number; generation: number }) {
      return available && token.context === context && token.generation === generation;
    },
  };
}
