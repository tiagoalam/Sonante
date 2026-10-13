import type { OnlineMetadataDetails } from "../types/onlineMetadata";
import { analyzeLocalAlbumTagForm, editLocalAlbumTagField } from "./localAlbumTagEditor.ts";
import type { LocalAlbumTagEditorDraft } from "./localAlbumTagEditor.ts";

export const transferFields = ["album", "album_artist", "date"] as const;
export type TransferField = typeof transferFields[number];
export type DateSource = "edition" | "first_release";
export type TransferValues = Partial<Record<TransferField, string>>;

export function validMetadataDate(value: string): boolean {
  if (!/^\d{4}(?:-\d{2}(?:-\d{2})?)?$/.test(value)) return false;
  const [year, month, day] = value.split("-").map(Number);
  if (year < 1 || year > 9999) return false;
  if (month === undefined) return true;
  if (month < 1 || month > 12) return false;
  if (day === undefined) return true;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  return day >= 1 && day <= [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
}

export function suggestionValue(details: OnlineMetadataDetails, field: TransferField, source: DateSource | null): string | null {
  if (field === "album") return details.title;
  if (field === "album_artist") return details.artist_credit;
  return source === "edition" ? details.edition_date : source === "first_release" ? details.first_release_date : null;
}

export function transferReason(value: string | null, date = false): "missing" | "empty" | "invalidDate" | null {
  if (value === null) return "missing";
  if (!value.trim()) return "empty";
  return date && !validMetadataDate(value) ? "invalidDate" : null;
}

// Only the explicitly selected fields pass through TAG-2's existing edit semantics.
export function applyOnlineMetadataValues(draft: LocalAlbumTagEditorDraft, values: TransferValues): LocalAlbumTagEditorDraft | null {
  const entries = Object.entries(values);
  if (!entries.length || analyzeLocalAlbumTagForm(draft).blocked || entries.some(([field, value]) =>
    !transferFields.includes(field as TransferField) || typeof value !== "string" || transferReason(value, field === "date"))) return null;
  return entries.reduce((next, [field, value]) => editLocalAlbumTagField(next, field as TransferField, value!), draft);
}

export interface MetadataTransferRequest {
  albumId: string;
  catalogVersion: unknown;
  inspection: LocalAlbumTagEditorDraft["inspection"];
  draft: LocalAlbumTagEditorDraft;
  editorRevision: string | undefined;
  details: OnlineMetadataDetails;
  values: TransferValues;
  isCurrent: () => boolean;
}
