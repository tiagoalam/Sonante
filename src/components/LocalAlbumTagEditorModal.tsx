import React, { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, Eye, LoaderCircle, RotateCcw, Undo2, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { localTagsService } from "../services/localTags";
import type {
  LocalAlbumTagBatchPreflightErrorCode,
  LocalAlbumTagInspection,
} from "../types/localTags";
import {
  analyzeLocalAlbumTagForm,
  buildLocalAlbumTagBatchPreflightRequest,
  clearMixedLocalAlbumTagField,
  createLocalAlbumTagEditorDraft,
  createLocalAlbumTagRequestGuard,
  type LocalAlbumTagPreviewView,
  editLocalAlbumTagField,
  localAlbumTagFields,
  localAlbumTagPreviewView,
  resetLocalAlbumTagField,
  type LocalAlbumTagEditorDraft,
} from "../utils/localAlbumTagEditor";

interface Props {
  albumId: string;
  displayName: string;
  available: boolean;
  catalogVersion: unknown;
  isAvailable: () => boolean;
  onClose: () => void;
}

function backendErrorCode(cause: unknown): LocalAlbumTagBatchPreflightErrorCode | null {
  if (!cause || typeof cause !== "object" || !("code" in cause)) return null;
  return typeof cause.code === "string"
    ? cause.code as LocalAlbumTagBatchPreflightErrorCode
    : null;
}

export const LocalAlbumTagEditorModal: React.FC<Props> = ({ albumId, displayName, available, catalogVersion, isAvailable, onClose }) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<LocalAlbumTagEditorDraft | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloading, setReloading] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState<{ view: LocalAlbumTagPreviewView; token: { context: number; generation: number } } | null>(null);
  const [inspectionContext, setInspectionContext] = useState<number | null>(null);
  const [stale, setStale] = useState(false);
  const [message, setMessage] = useState<{ kind: "error" | "warning"; text: string } | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const guard = useRef(createLocalAlbumTagRequestGuard()).current;
  const context = guard.syncContext(albumId, catalogVersion, available);
  const snapshotCurrent = available && draft?.inspection.album_id === albumId && inspectionContext === context;
  const mustReload = stale || !snapshotCurrent;

  const analysis = useMemo(() => draft ? analyzeLocalAlbumTagForm(draft) : null, [draft]);
  const previewView = preview && snapshotCurrent && !stale && guard.isCurrent(preview.token) ? preview.view : null;

  useEffect(() => {
    setPreview(null);
    setLoading(false);
    setReloading(false);
    setPreviewing(false);
    setMessage(null);
  }, [context]);

  const loadInspection = async (isReload: boolean) => {
    if (!available || !isAvailable()) {
      setMessage({ kind: "warning", text: t("albumTagEditor.unavailable") });
      setLoading(false);
      return;
    }
    const request = guard.capture();
    setPreview(null);
    setStale(true);
    if (isReload) setReloading(true);
    else setLoading(true);
    setMessage(null);
    try {
      const inspection: LocalAlbumTagInspection = await localTagsService.getAlbumInspection(albumId);
      if (!guard.isCurrent(request) || !isAvailable()) return;
      if (inspection.album_id !== albumId) throw new Error("inspection_mismatch");
      setDraft(createLocalAlbumTagEditorDraft(inspection));
      setInspectionContext(request.context);
      setPreview(null);
      setStale(false);
    } catch {
      if (!guard.isCurrent(request) || !isAvailable()) return;
      setMessage({ kind: "error", text: t("albumTagEditor.loadError") });
    } finally {
      if (guard.isCurrent(request)) {
        setLoading(false);
        setReloading(false);
      }
    }
  };

  useEffect(() => {
    // A changed album must not silently discard an existing draft.
    if (!draft) void loadInspection(false);
    return () => {
      guard.invalidate();
    };
  }, [albumId]);

  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    dialogRef.current?.focus();
    return () => previousFocus?.focus();
  }, []);

  const requestClose = () => {
    if (previewing) return;
    if (analysis?.dirty && !window.confirm(t("albumTagEditor.discardConfirm"))) return;
    guard.invalidate();
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
  }, [analysis?.dirty, previewing, t]);

  const changeField = (field: typeof localAlbumTagFields[number], value: string) => {
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? editLocalAlbumTagField(current, field, value) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const clearAll = (field: typeof localAlbumTagFields[number]) => {
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? clearMixedLocalAlbumTagField(current, field) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const undoField = (field: typeof localAlbumTagFields[number]) => {
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? resetLocalAlbumTagField(current, field) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const reload = () => {
    if (analysis?.dirty && !window.confirm(t("albumTagEditor.reloadConfirm"))) return;
    void loadInspection(true);
  };

  const requestPreview = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!draft || !analysis?.dirty || analysis.blocked || mustReload || previewing || reloading) return;
    if (!available || !isAvailable()) {
      guard.invalidate();
      setPreview(null);
      setStale(true);
      setMessage({ kind: "warning", text: t("albumTagEditor.unavailable") });
      return;
    }
    const request = guard.capture();
    setPreviewing(true);
    setPreview(null);
    setMessage(null);
    try {
      const plan = await localTagsService.preflightAlbumBatch(
        buildLocalAlbumTagBatchPreflightRequest(draft),
      );
      if (!guard.isCurrent(request) || !isAvailable()) return;
      let view: LocalAlbumTagPreviewView;
      try {
        view = localAlbumTagPreviewView(plan, draft.inspection);
      } catch {
        setStale(true);
        setMessage({ kind: "warning", text: t("albumTagEditor.invalidPlan") });
        return;
      }
      setPreview({ view, token: request });
      if (!plan.applicable) {
        const key = plan.blocked_reason === "cue_virtual"
          ? "albumTagEditor.cueBlocked"
          : "albumTagEditor.inspectionBlocked";
        setMessage({ kind: "warning", text: t(key) });
      }
    } catch (cause) {
      if (!guard.isCurrent(request) || !isAvailable()) return;
      const code = backendErrorCode(cause);
      if (code === "membership_changed" || code === "revision_conflict" || code === "invalid_snapshot") {
        setStale(true);
        setMessage({ kind: "warning", text: t("albumTagEditor.stale") });
      } else if (code === "album_not_found") {
        setMessage({ kind: "error", text: t("albumTagEditor.albumMissing") });
      } else {
        setMessage({ kind: "error", text: t("albumTagEditor.previewError") });
      }
    } finally {
      if (guard.isCurrent(request)) setPreviewing(false);
    }
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
        aria-labelledby="local-album-tag-editor-title"
        className="flex max-h-[92vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-[#333333] bg-[#171717] shadow-2xl"
      >
        <header className="flex items-center justify-between border-b border-[#292929] px-5 py-4">
          <div className="min-w-0">
            <h2 id="local-album-tag-editor-title" className="text-base font-bold text-white">
              {t("albumTagEditor.title")}
            </h2>
            <p className="mt-1 truncate text-xs text-[#777777]">{displayName}</p>
          </div>
          <button
            type="button"
            onClick={requestClose}
            disabled={previewing}
            aria-label={t("albumTagEditor.close")}
            className="rounded-lg p-2 text-[#888888] hover:bg-[#252525] hover:text-white disabled:opacity-50"
          >
            <X size={18} />
          </button>
        </header>

        {draft && mustReload && (
          <p role="status" className="mx-5 mt-4 text-xs text-amber-200">
            {t(snapshotCurrent ? "albumTagEditor.stale" : "albumTagEditor.unavailable")}
          </p>
        )}
        {message && (
          <div
            role={message.kind === "error" ? "alert" : "status"}
            className={`mx-5 mt-4 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs ${
              message.kind === "warning"
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
            <span>{t("albumTagEditor.loading")}</span>
          </div>
        ) : draft ? (
          <form onSubmit={requestPreview} className="flex min-h-0 flex-1 flex-col">
            <div className="overflow-y-auto px-5 py-4">
              <div className="mb-4 flex flex-wrap gap-x-4 gap-y-1 text-[11px] uppercase tracking-wider text-[#777777]">
                <span>{t("albumTagEditor.physicalTracks", { count: draft.inspection.physical_track_count })}</span>
                {draft.inspection.cue_virtual_count > 0 && (
                  <span className="text-amber-300">
                    {t("albumTagEditor.cueTracks", { count: draft.inspection.cue_virtual_count })}
                  </span>
                )}
              </div>

              {analysis?.blocked && (
                <div className="mb-4 rounded-lg border border-amber-400/25 bg-amber-400/10 px-3 py-2 text-xs text-amber-100">
                  {t(analysis.blockedReason === "cue" ? "albumTagEditor.cueBlocked"
                    : analysis.blockedReason === "format" ? "albumTagEditor.formatBlocked"
                    : "albumTagEditor.inspectionBlocked")}
                </div>
              )}

              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                {localAlbumTagFields.map((field, index) => {
                  const fieldDraft = draft.fields[field];
                  const state = fieldDraft.summary.state;
                  const disabled = Boolean(analysis?.blocked || !snapshotCurrent || reloading);
                  return (
                    <div key={field} className="space-y-1.5">
                      <label htmlFor={`local-album-tag-${field}`} className="text-xs text-[#BBBBBB]">
                        {t(`tagEditor.fields.${field}`)}
                      </label>
                      <input
                        id={`local-album-tag-${field}`}
                        autoFocus={index === 0}
                        type="text"
                        value={fieldDraft.value}
                        placeholder={state === "mixed"
                          ? t("albumTagEditor.states.mixed")
                          : state === "absent"
                            ? t("albumTagEditor.states.absent")
                            : ""}
                        onChange={(event) => changeField(field, event.target.value)}
                        disabled={disabled}
                        className="w-full rounded-lg border border-[#333333] bg-[#111111] px-3 py-2 text-sm text-white outline-none placeholder:text-[#666666] focus:border-[#E5A00D] disabled:opacity-60"
                      />
                      <div className="flex min-h-5 items-center justify-between gap-2 text-[11px]">
                        <span className={state === "indeterminate" || state === "not_applicable" ? "text-amber-300" : "text-[#666666]"}>
                          {fieldDraft.clearAll
                            ? t("albumTagEditor.removeForAll")
                            : t(`albumTagEditor.states.${state}`)}
                        </span>
                        {state === "mixed" && (
                          <button
                            type="button"
                            onClick={() => fieldDraft.clearAll ? undoField(field) : clearAll(field)}
                            disabled={disabled}
                            className="flex items-center gap-1 text-[#999999] hover:text-[#E5A00D] disabled:opacity-50"
                          >
                            {fieldDraft.clearAll ? <Undo2 size={12} /> : <X size={12} />}
                            {t(fieldDraft.clearAll ? "albumTagEditor.undoClear" : "albumTagEditor.clearForAll")}
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>

              {previewView && (
                <div className={`mt-5 rounded-xl border px-4 py-3 ${
                  previewView.applicable
                    ? "border-emerald-400/25 bg-emerald-400/10"
                    : "border-amber-400/25 bg-amber-400/10"
                }`}>
                  <p className={`text-sm font-bold ${previewView.applicable ? "text-emerald-200" : "text-amber-100"}`}>
                    {t(previewView.applicable ? "albumTagEditor.previewReady" : "albumTagEditor.previewBlocked")}
                  </p>
                  <p className="mt-1 text-xs text-[#BBBBBB]">
                    {t("albumTagEditor.previewSummary", {
                      changeCount: previewView.changeCount,
                      unchangedCount: previewView.unchangedCount,
                    })}
                  </p>
                  <ul className="mt-3 space-y-3 text-xs">
                    {previewView.files.map((file) => (
                      <li key={file.uri} className="rounded-lg border border-[#333333] p-3">
                        <p className="break-all font-mono">{file.uri}</p>
                        {file.blocked ? (
                          <p className="mt-1 text-amber-200">{t("albumTagEditor.blockedFile")}</p>
                        ) : file.changes.length === 0 ? (
                          <p className="mt-1 text-[#888888]">{t("albumTagEditor.unchangedFile")}</p>
                        ) : (
                          <dl className="mt-2 space-y-2">
                            {file.changes.map((change) => (
                              <div key={change.field}>
                                <dt className="font-bold">{t(`tagEditor.fields.${change.field}`)}</dt>
                                <dd className="whitespace-pre-wrap break-all">
                                  {t("albumTagEditor.before")}: {change.before === null ? t("albumTagEditor.absentValue") : JSON.stringify(change.before)}
                                </dd>
                                <dd className="whitespace-pre-wrap break-all">
                                  {t("albumTagEditor.proposed")}: {change.proposed === null ? t("albumTagEditor.removed") : JSON.stringify(change.proposed)}
                                </dd>
                              </div>
                            ))}
                          </dl>
                        )}
                      </li>
                    ))}
                  </ul>
                  {previewView.applicable && (
                    <p className="mt-2 text-[11px] text-[#888888]">{t("albumTagEditor.previewOnly")}</p>
                  )}
                </div>
              )}
            </div>

            <footer className="flex items-center justify-between gap-3 border-t border-[#292929] px-5 py-4">
              <div>
                <button
                  type="button"
                  onClick={reload}
                  disabled={!available || previewing || reloading}
                  className="flex items-center gap-2 rounded-lg border border-amber-400/30 px-3 py-2 text-xs font-bold text-amber-200 hover:bg-amber-400/10 disabled:opacity-50"
                >
                  <RotateCcw size={14} className={reloading ? "animate-spin" : ""} />
                  {t("albumTagEditor.reload")}
                </button>
              </div>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={requestClose}
                  disabled={previewing}
                  className="rounded-lg border border-[#333333] px-4 py-2 text-sm font-semibold text-white hover:bg-[#252525] disabled:opacity-50"
                >
                  {t("albumTagEditor.cancel")}
                </button>
                <button
                  type="submit"
                  disabled={!analysis?.dirty || analysis.blocked || mustReload || previewing || reloading}
                  className="flex items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black hover:bg-[#F5B01D] disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {previewing ? <LoaderCircle size={16} className="animate-spin" /> : <Eye size={16} />}
                  {t(previewing ? "albumTagEditor.previewing" : "albumTagEditor.preview")}
                </button>
              </div>
            </footer>
          </form>
        ) : (
          <div className="min-h-32 p-5">
            <button type="button" disabled={!available || loading || reloading} onClick={reload}
              className="text-sm text-amber-200 disabled:opacity-40">{t("albumTagEditor.reload")}</button>
          </div>
        )}
      </div>
    </div>
  );
};
