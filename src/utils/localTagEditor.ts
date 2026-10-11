import type {
  LocalTrackTags,
  LocalTrackTagUpdate,
  LocalTrackTagUpdateResult,
  TagFieldUpdate,
} from "../types/localTags";

export interface LocalTagFormValues {
  title: string;
  artist: string;
  album: string;
  album_artist: string;
  genre: string;
  date: string;
  track_number: string;
  track_total: string;
  disc_number: string;
  disc_total: string;
}

export type LocalTagNumberField =
  | "track_number"
  | "track_total"
  | "disc_number"
  | "disc_total";

export type LocalTagValidationError = "positive_integer" | "total_less_than_number";

export interface LocalTagFormAnalysis {
  patch: LocalTrackTagUpdate;
  dirty: boolean;
  valid: boolean;
  errors: Partial<Record<LocalTagNumberField, LocalTagValidationError>>;
}

export interface LocalTagEditorDraft {
  initial: LocalTrackTags;
  values: LocalTagFormValues;
  revisionConflict: boolean;
}

const textFields = ["title", "artist", "album", "album_artist", "genre", "date"] as const;
const numberFields = ["track_number", "track_total", "disc_number", "disc_total"] as const;

export function localTagsToForm(tags: LocalTrackTags): LocalTagFormValues {
  return {
    title: tags.title ?? "",
    artist: tags.artist ?? "",
    album: tags.album ?? "",
    album_artist: tags.album_artist ?? "",
    genre: tags.genre ?? "",
    date: tags.date ?? "",
    track_number: tags.track_number?.toString() ?? "",
    track_total: tags.track_total?.toString() ?? "",
    disc_number: tags.disc_number?.toString() ?? "",
    disc_total: tags.disc_total?.toString() ?? "",
  };
}

export function createLocalTagEditorDraft(tags: LocalTrackTags): LocalTagEditorDraft {
  return { initial: tags, values: localTagsToForm(tags), revisionConflict: false };
}

export function markLocalTagRevisionConflict(draft: LocalTagEditorDraft): LocalTagEditorDraft {
  return { ...draft, revisionConflict: true };
}

export function parsePositiveTagInteger(value: string): number | null {
  if (!/^[1-9]\d*$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function textUpdate(initial: string | null, current: string): TagFieldUpdate<string> {
  if (current === (initial ?? "")) return { action: "unchanged" };
  if (current === "") return initial === null ? { action: "unchanged" } : { action: "remove" };
  return { action: "set", value: current };
}

function numberUpdate(initial: number | null, current: string): TagFieldUpdate<number> {
  if (current === "") return initial === null ? { action: "unchanged" } : { action: "remove" };
  const parsed = parsePositiveTagInteger(current);
  if (parsed === null || parsed === initial) return { action: "unchanged" };
  return { action: "set", value: parsed };
}

function changed(update: TagFieldUpdate<unknown>): boolean {
  return update.action !== "unchanged";
}

export function analyzeLocalTagForm(
  initial: LocalTrackTags,
  values: LocalTagFormValues,
): LocalTagFormAnalysis {
  const patch = {} as LocalTrackTagUpdate;
  for (const field of textFields) patch[field] = textUpdate(initial[field], values[field]);
  for (const field of numberFields) patch[field] = numberUpdate(initial[field], values[field]);

  const errors: Partial<Record<LocalTagNumberField, LocalTagValidationError>> = {};
  const parsed: Partial<Record<LocalTagNumberField, number>> = {};
  for (const field of numberFields) {
    if (values[field] === "") continue;
    const value = parsePositiveTagInteger(values[field]);
    if (value === null) errors[field] = "positive_integer";
    else parsed[field] = value;
  }
  if (
    parsed.track_number !== undefined
    && parsed.track_total !== undefined
    && parsed.track_total < parsed.track_number
  ) {
    errors.track_total = "total_less_than_number";
  }
  if (
    parsed.disc_number !== undefined
    && parsed.disc_total !== undefined
    && parsed.disc_total < parsed.disc_number
  ) {
    errors.disc_total = "total_less_than_number";
  }

  const dirty = textFields.some((field) => changed(patch[field]))
    || numberFields.some((field) => {
      if (errors[field]) return values[field] !== (initial[field]?.toString() ?? "");
      return changed(patch[field]);
    });
  return { patch, dirty, valid: Object.keys(errors).length === 0, errors };
}

export type LocalTagSaveOutcome = "saved" | "saved_refresh_failed" | "not_committed";

export function localTagSaveOutcome(result: LocalTrackTagUpdateResult): LocalTagSaveOutcome {
  if (!result.committed) return "not_committed";
  return result.mpd_refresh.success ? "saved" : "saved_refresh_failed";
}
