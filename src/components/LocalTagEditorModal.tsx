import React, { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, LoaderCircle, RotateCcw, Save, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { localTagsService } from "../services/localTags";
import type { LocalTrackTags, LocalTrackTagsErrorCode } from "../types/localTags";
import {
  analyzeLocalTagForm,
  createLocalTagEditorDraft,
  localTagSaveOutcome,
  markLocalTagRevisionConflict,
  type LocalTagEditorDraft,
  type LocalTagFormValues,
  type LocalTagNumberField,
} from "../utils/localTagEditor";

interface Props {
  path: string;
  displayName: string;
  onClose: () => void;
  onSaved: (tags: LocalTrackTags) => void;
}

const textFields = ["title", "artist", "album", "album_artist", "genre", "date"] as const;
const numberFields: LocalTagNumberField[] = [
  "track_number",
  "track_total",
  "disc_number",
  "disc_total",
];

function backendErrorCode(cause: unknown): LocalTrackTagsErrorCode | null {
  if (!cause || typeof cause !== "object" || !("code" in cause)) return null;
  return typeof cause.code === "string" ? cause.code as LocalTrackTagsErrorCode : null;
}

export const LocalTagEditorModal: React.FC<Props> = ({ path, displayName, onClose, onSaved }) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<LocalTagEditorDraft | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloading, setReloading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ kind: "error" | "warning" | "success"; text: string } | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const requestRef = useRef(0);
  const closeTimerRef = useRef<number | null>(null);
  const saveInFlightRef = useRef(false);

  const analysis = useMemo(
    () => draft ? analyzeLocalTagForm(draft.initial, draft.values) : null,
    [draft],
  );
  const writable = Boolean(draft?.initial.writable && draft.initial.editable);

  const loadTags = async (preserveDraft: boolean) => {
    const request = ++requestRef.current;
    if (preserveDraft) setReloading(true);
    else setLoading(true);
    setMessage(null);
    try {
      const tags = await localTagsService.get(path);
      if (request !== requestRef.current) return;
      setDraft(createLocalTagEditorDraft(tags));
      if (!tags.writable || !tags.editable) {
        setMessage({ kind: "warning", text: t("tagEditor.readOnly") });
      }
    } catch (cause) {
      if (request !== requestRef.current) return;
      const code = backendErrorCode(cause);
      const key = code === "cue_virtual"
        ? "tagEditor.cueUnsupported"
        : code === "unsupported_format"
          ? "tagEditor.unsupported"
          : code === "file_not_found"
            ? "tagEditor.fileMissing"
            : "tagEditor.loadError";
      setMessage({ kind: code === "cue_virtual" || code === "unsupported_format" ? "warning" : "error", text: t(key) });
    } finally {
      if (request === requestRef.current) {
        setLoading(false);
        setReloading(false);
      }
    }
  };

  useEffect(() => {
    void loadTags(false);
    return () => {
      requestRef.current += 1;
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    };
  }, [path]);

  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    dialogRef.current?.focus();
    return () => previousFocus?.focus();
  }, []);

  const requestClose = () => {
    if (saving) return;
    if (analysis?.dirty && !window.confirm(t("tagEditor.discardConfirm"))) return;
    onClose();
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose();
        return;
      }
      if (event.key !== "Tab" || !dialogRef.current) return;
      const focusable = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(
        "button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex='-1'])",
      ));
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [analysis?.dirty, saving, t]);

  const setValue = (field: keyof LocalTagFormValues, value: string) => {
    setDraft((current) => current ? {
      ...current,
      values: { ...current.values, [field]: value },
    } : current);
    if (!draft?.revisionConflict) setMessage(null);
  };

  const saveTags = async (event: React.FormEvent) => {
    event.preventDefault();
    if (
      !draft
      || !analysis
      || !analysis.dirty
      || !analysis.valid
      || !writable
      || draft.revisionConflict
      || saveInFlightRef.current
    ) return;
    saveInFlightRef.current = true;
    setSaving(true);
    setMessage(null);
    try {
      const result = await localTagsService.update(path, draft.initial.revision, analysis.patch);
      const outcome = localTagSaveOutcome(result);
      if (outcome === "not_committed") {
        setMessage({ kind: "error", text: t("tagEditor.saveError") });
        return;
      }

      setDraft(createLocalTagEditorDraft(result.tags));
      onSaved(result.tags);
      if (outcome === "saved_refresh_failed") {
        setMessage({ kind: "warning", text: t("tagEditor.savedRefreshFailed") });
      } else {
        setMessage({ kind: "success", text: t("tagEditor.saved") });
        closeTimerRef.current = window.setTimeout(onClose, 700);
      }
    } catch (cause) {
      const code = backendErrorCode(cause);
      if (code === "revision_conflict") {
        setDraft((current) => current ? markLocalTagRevisionConflict(current) : current);
        setMessage({ kind: "warning", text: t("tagEditor.revisionConflict") });
      } else if (code === "invalid_tag_update") {
        setMessage({ kind: "error", text: t("tagEditor.validation.backend") });
      } else if (code === "cue_virtual") {
        setMessage({ kind: "warning", text: t("tagEditor.cueUnsupported") });
      } else if (code === "unsupported_format") {
        setMessage({ kind: "warning", text: t("tagEditor.unsupported") });
      } else {
        setMessage({ kind: "error", text: t("tagEditor.saveError") });
      }
    } finally {
      saveInFlightRef.current = false;
      setSaving(false);
    }
  };

  const validationMessage = (field: LocalTagNumberField) => {
    const error = analysis?.errors[field];
    if (!error) return null;
    return t(error === "positive_integer"
      ? "tagEditor.validation.positiveInteger"
      : "tagEditor.validation.totalTooSmall");
  };

  return (
    <div
      className="fixed inset-0 z-60 flex items-center justify-center bg-black/75 p-4 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="local-tag-editor-title"
        className="flex max-h-[92vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-[#333333] bg-[#171717] shadow-2xl"
      >
        <header className="flex items-center justify-between border-b border-[#292929] px-5 py-4">
          <div className="min-w-0">
            <h2 id="local-tag-editor-title" className="text-base font-bold text-white">
              {t("tagEditor.title")}
            </h2>
            <p className="mt-1 truncate text-xs text-[#777777]">{displayName}</p>
          </div>
          <button
            type="button"
            onClick={requestClose}
            disabled={saving}
            aria-label={t("tagEditor.close")}
            className="rounded-lg p-2 text-[#888888] hover:bg-[#252525] hover:text-white disabled:opacity-50"
          >
            <X size={18} />
          </button>
        </header>

        {message && (
          <div
            role={message.kind === "error" ? "alert" : "status"}
            className={`mx-5 mt-4 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs ${
              message.kind === "success"
                ? "border-emerald-400/25 bg-emerald-400/10 text-emerald-200"
                : message.kind === "warning"
                  ? "border-amber-400/25 bg-amber-400/10 text-amber-100"
                  : "border-red-400/25 bg-red-400/10 text-red-200"
            }`}
          >
            {message.kind === "warning" && <AlertTriangle size={15} className="mt-0.5 shrink-0" />}
            <span>{message.text}</span>
          </div>
        )}

        {loading && !draft ? (
          <div className="flex min-h-64 items-center justify-center gap-2 text-sm text-[#888888]">
            <LoaderCircle size={20} className="animate-spin" />
            <span>{t("tagEditor.loading")}</span>
          </div>
        ) : draft ? (
          <form onSubmit={saveTags} className="flex min-h-0 flex-1 flex-col">
            <div className="overflow-y-auto px-5 py-4">
              <div className="mb-4 flex items-center gap-2 text-[11px] uppercase tracking-wider text-[#777777]">
                <span>{draft.initial.format.toUpperCase()}</span>
                <span>•</span>
                <span className={writable ? "text-emerald-300" : "text-amber-300"}>
                  {t(writable ? "tagEditor.editable" : "tagEditor.readOnlyLabel")}
                </span>
              </div>

              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                {textFields.map((field, index) => (
                  <label key={field} htmlFor={`local-tag-${field}`} className="space-y-1.5 text-xs text-[#BBBBBB]">
                    <span>{t(`tagEditor.fields.${field}`)}</span>
                    <input
                      id={`local-tag-${field}`}
                      autoFocus={index === 0}
                      type="text"
                      value={draft.values[field]}
                      onChange={(event) => setValue(field, event.target.value)}
                      disabled={!writable || saving}
                      className="w-full rounded-lg border border-[#333333] bg-[#111111] px-3 py-2 text-sm text-white outline-none focus:border-[#E5A00D] disabled:opacity-60"
                    />
                  </label>
                ))}
                {numberFields.map((field) => {
                  const fieldError = validationMessage(field);
                  return (
                    <label key={field} htmlFor={`local-tag-${field}`} className="space-y-1.5 text-xs text-[#BBBBBB]">
                      <span>{t(`tagEditor.fields.${field}`)}</span>
                      <input
                        id={`local-tag-${field}`}
                        type="number"
                        min="1"
                        step="1"
                        inputMode="numeric"
                        value={draft.values[field]}
                        onChange={(event) => setValue(field, event.target.value)}
                        disabled={!writable || saving}
                        aria-invalid={Boolean(fieldError)}
                        className="w-full rounded-lg border border-[#333333] bg-[#111111] px-3 py-2 text-sm text-white outline-none focus:border-[#E5A00D] disabled:opacity-60"
                      />
                      {fieldError && <span className="block text-[11px] text-red-300">{fieldError}</span>}
                    </label>
                  );
                })}
              </div>
            </div>

            <footer className="flex items-center justify-between gap-3 border-t border-[#292929] px-5 py-4">
              <div>
                {draft.revisionConflict && (
                  <button
                    type="button"
                    onClick={() => void loadTags(true)}
                    disabled={saving || reloading}
                    className="flex items-center gap-2 rounded-lg border border-amber-400/30 px-3 py-2 text-xs font-bold text-amber-200 hover:bg-amber-400/10 disabled:opacity-50"
                  >
                    <RotateCcw size={14} className={reloading ? "animate-spin" : ""} />
                    {t("tagEditor.reload")}
                  </button>
                )}
              </div>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={requestClose}
                  disabled={saving}
                  className="rounded-lg border border-[#333333] px-4 py-2 text-sm font-semibold text-white hover:bg-[#252525] disabled:opacity-50"
                >
                  {t("tagEditor.cancel")}
                </button>
                <button
                  type="submit"
                  disabled={!analysis?.dirty || !analysis.valid || !writable || draft.revisionConflict || saving || reloading}
                  className="flex items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black hover:bg-[#F5B01D] disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {saving ? <LoaderCircle size={16} className="animate-spin" /> : <Save size={16} />}
                  {t(saving ? "tagEditor.saving" : "tagEditor.save")}
                </button>
              </div>
            </footer>
          </form>
        ) : (
          <div className="min-h-32" />
        )}
      </div>
    </div>
  );
};
