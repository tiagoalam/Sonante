import React, { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, Eye, LoaderCircle, RotateCcw, Undo2, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { localTagsService } from "../services/localTags";
import { applyOnlineMetadataValues } from "../utils/onlineMetadataTransfer";
import type { MetadataTransferRequest } from "../utils/onlineMetadataTransfer";
import { OnlineAlbumMetadataPanel } from "./OnlineAlbumMetadataPanel";
import type { DoctorReviewContext } from "../utils/libraryDoctorReview";
import type {
  LocalAlbumTagBatchPreflightErrorCode,
  LocalAlbumTagBatchPreflightRequest,
  LocalAlbumTagBatchResult,
  LocalAlbumTagInspection,
} from "../types/localTags";
import {
  analyzeLocalAlbumTagForm,
  buildLocalAlbumTagBatchPreflightRequest,
  clearMixedLocalAlbumTagField,
  createLocalAlbumTagEditorDraft,
  createLocalAlbumTagRequestGuard,
  freezeLocalAlbumTagBatchRequest,
  validateLocalAlbumTagBatchResult,
  localAlbumTagApplyErrorKind,
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
  onBatchFinished?: (result: LocalAlbumTagBatchResult | null) => void;
  reviewContext?: DoctorReviewContext | null;
}

function backendErrorCode(cause: unknown): LocalAlbumTagBatchPreflightErrorCode | null {
  if (!cause || typeof cause !== "object" || !("code" in cause)) return null;
  return typeof cause.code === "string"
    ? cause.code as LocalAlbumTagBatchPreflightErrorCode
    : null;
}

export const LocalAlbumTagEditorModal: React.FC<Props> = ({ albumId, displayName, available, catalogVersion, isAvailable, onClose, onBatchFinished, reviewContext }) => {
  const { t } = useTranslation();
  const review = reviewContext?.albumId === albumId ? reviewContext : null;
  const [draft, setDraft] = useState<LocalAlbumTagEditorDraft | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloading, setReloading] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState<{ view: LocalAlbumTagPreviewView; request: LocalAlbumTagBatchPreflightRequest; token: { context: number; generation: number } } | null>(null);
  const [applying, setApplying] = useState(false);
  const [result, setResult] = useState<LocalAlbumTagBatchResult | null>(null);
  const [applyIssue, setApplyIssue] = useState<"conflict" | "unknown" | null>(null);
  const applyInFlight = useRef(false);
  const consumed = useRef(false);
  const previewRef = useRef(preview);
  previewRef.current = preview;
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
    if (applyInFlight.current) return;
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
      consumed.current = false;
      setResult(null);
      setApplyIssue(null);
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
    if (applyInFlight.current || previewing) return;
    if ((consumed.current || analysis?.dirty) && !window.confirm(t(consumed.current
      ? "albumTagEditor.resultCloseConfirm" : "albumTagEditor.discardConfirm"))) return;
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
  }, [analysis?.dirty, previewing, applying, result, applyIssue, t]);

  const editorRevision = guard.version();
  const metadataAvailable = () => guard.version() === editorRevision && snapshotCurrent && !mustReload
    && !loading && !reloading && !previewing && !applying && !applyInFlight.current && !consumed.current && isAvailable();
  const transferMetadata = (request: MetadataTransferRequest): boolean => {
    if (!metadataAvailable() || !request.isCurrent() || !draft || analysis?.blocked
      || request.albumId !== albumId || request.catalogVersion !== catalogVersion || request.inspection !== draft.inspection
      || request.draft !== draft || request.editorRevision !== guard.version()) return false;
    const next = applyOnlineMetadataValues(draft, request.values);
    if (!next) return false;
    guard.invalidate();
    previewRef.current = null;
    setPreview(null);
    setDraft(current => current === request.draft ? next : current);
    setMessage(null);
    return true;
  };

  const changeField = (field: typeof localAlbumTagFields[number], value: string) => {
    if (applyInFlight.current || consumed.current) return;
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? editLocalAlbumTagField(current, field, value) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const clearAll = (field: typeof localAlbumTagFields[number]) => {
    if (applyInFlight.current || consumed.current) return;
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? clearMixedLocalAlbumTagField(current, field) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const undoField = (field: typeof localAlbumTagFields[number]) => {
    if (applyInFlight.current || consumed.current) return;
    guard.invalidate();
    setPreviewing(false);
    setDraft((current) => current ? resetLocalAlbumTagField(current, field) : current);
    setPreview(null);
    if (!stale) setMessage(null);
  };

  const reload = () => {
    if (applyInFlight.current) return;
    if ((consumed.current || analysis?.dirty) && !window.confirm(t(consumed.current
      ? "albumTagEditor.resultReloadConfirm" : "albumTagEditor.reloadConfirm"))) return;
    void loadInspection(true);
  };

  const requestPreview = async (event: React.FormEvent) => {
    event.preventDefault();
    if (applyInFlight.current || consumed.current) return;
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
      const exactRequest = freezeLocalAlbumTagBatchRequest(buildLocalAlbumTagBatchPreflightRequest(draft));
      const plan = await localTagsService.preflightAlbumBatch(exactRequest);
      if (!guard.isCurrent(request) || !isAvailable()) return;
      let view: LocalAlbumTagPreviewView;
      try {
        view = localAlbumTagPreviewView(plan, draft.inspection);
      } catch {
        setStale(true);
        setMessage({ kind: "warning", text: t("albumTagEditor.invalidPlan") });
        return;
      }
      setPreview({ view, request: exactRequest, token: request });
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

  const requestApply = async () => {
    const approved = previewRef.current;
    if (applyInFlight.current || consumed.current || !approved || !approved.view.canApply
      || mustReload || !guard.isCurrent(approved.token) || !available || !isAvailable()) return;
    // Set synchronously before confirmation: repeated clicks cannot confirm/send twice.
    applyInFlight.current = true;
    if (!window.confirm(t("albumTagEditor.applyConfirm", { count: approved.view.changeCount }))) {
      applyInFlight.current = false;
      return;
    }
    if (previewRef.current !== approved || !guard.isCurrent(approved.token) || !isAvailable()) {
      applyInFlight.current = false;
      setPreview(null);
      setStale(true);
      return;
    }
    consumed.current = true;
    setApplying(true);
    setApplyIssue(null);
    setMessage(null);
    previewRef.current = null;
    setPreview(null);
    guard.invalidate();
    setInspectionContext(null);
    setStale(true);
    let outcome: LocalAlbumTagBatchResult | null = null;
    try {
      // Never send the plan. Backend repeats preflight and revision checks on this request.
      const received = await localTagsService.applyAlbumBatch(approved.request);
      validateLocalAlbumTagBatchResult(received, approved.request);
      outcome = received;
      // Apply responses deliberately do not use the catalog/request guard: writes already ran.
      setResult(received);
    } catch (cause) {
      setApplyIssue(localAlbumTagApplyErrorKind(cause));
    } finally {
      applyInFlight.current = false;
      setApplying(false);
      // This callback only queries/reconciles the existing catalog; it never rescans MPD.
      try { onBatchFinished?.(outcome); }
      catch { setMessage({ kind: "warning", text: t("albumTagEditor.catalogNotifyFailed") }); }
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
        className="flex max-h-[92vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#171717)] shadow-2xl"
      >
        <header className="flex items-center justify-between border-b border-[color:var(--theme-border-soft,#292929)] px-5 py-4">
          <div className="min-w-0">
            <h2 id="local-album-tag-editor-title" className="text-base font-bold text-[color:var(--theme-fg-primary,#ffffff)]">
              {t("albumTagEditor.title")}
            </h2>
            <p className="mt-1 truncate text-xs text-[color:var(--theme-fg-muted,#777777)]">{displayName}</p>
          </div>
          <button
            type="button"
            onClick={requestClose}
            disabled={applying || previewing}
            aria-label={t("albumTagEditor.close")}
            className="rounded-lg p-2 text-[color:var(--theme-fg-muted,#888888)] hover:bg-[var(--theme-surface-raised,#252525)] hover:text-[color:var(--theme-fg-primary,#ffffff)] disabled:opacity-50"
          >
            <X size={18} />
          </button>
        </header>

        {review && <p id="local-album-tag-doctor-context" role="note"
          className="mx-5 mt-4 rounded-lg border border-[color:var(--theme-border-info,var(--color-sky-400))]/25 bg-sky-400/10 px-3 py-2 text-xs text-[color:var(--theme-fg-info,var(--color-sky-100))]">
          {t("libraryDoctor.review.context", { finding: t(`libraryDoctor.codes.${review.code}`) })}
        </p>}

        {draft && mustReload && !applying && !result && !applyIssue && (
          <p role="status" className="mx-5 mt-4 text-xs text-[color:var(--theme-fg-accent,var(--color-amber-200))]">
            {t(snapshotCurrent ? "albumTagEditor.stale" : "albumTagEditor.unavailable")}
          </p>
        )}
        {applying && <p role="status" className="mx-5 mt-4 text-sm text-[color:var(--theme-fg-accent,var(--color-amber-200))]">{t("albumTagEditor.applying")}</p>}
        {applyIssue && <p role="alert" className="mx-5 mt-4 text-sm text-[color:var(--theme-fg-accent,var(--color-amber-200))]">
          {t(applyIssue === "conflict" ? "albumTagEditor.applyConflict" : "albumTagEditor.applyUnknown")}
        </p>}
        {message && (
          <div
            role={message.kind === "error" ? "alert" : "status"}
            className={`mx-5 mt-4 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs ${
              message.kind === "warning"
                ? "border-[color:var(--theme-border-accent,var(--color-amber-400))]/25 bg-amber-400/10 text-[color:var(--theme-fg-accent,var(--color-amber-100))]"
                : "border-[color:var(--theme-border-error,var(--color-red-400))]/25 bg-red-400/10 text-[color:var(--theme-fg-error,var(--color-red-200))]"
            }`}
          >
            {message.kind === "warning" && <AlertTriangle size={15} className="mt-0.5 shrink-0" />}
            <span>{message.text}</span>
          </div>
        )}

        {loading && !draft ? (
          <div className="flex min-h-64 items-center justify-center gap-2 text-sm text-[color:var(--theme-fg-muted,#888888)]">
            <LoaderCircle size={20} className="animate-spin" />
            <span>{t("albumTagEditor.loading")}</span>
          </div>
        ) : draft ? (
          <form onSubmit={requestPreview} className="flex min-h-0 flex-1 flex-col">
            <div className="overflow-y-auto px-5 py-4">
              <div className="mb-4 flex flex-wrap gap-x-4 gap-y-1 text-[11px] uppercase tracking-wider text-[color:var(--theme-fg-muted,#777777)]">
                <span>{t("albumTagEditor.physicalTracks", { count: draft.inspection.physical_track_count })}</span>
                {draft.inspection.cue_virtual_count > 0 && (
                  <span className="text-[color:var(--theme-fg-accent,var(--color-amber-300))]">
                    {t("albumTagEditor.cueTracks", { count: draft.inspection.cue_virtual_count })}
                  </span>
                )}
              </div>

              {analysis?.blocked && (
                <div className="mb-4 rounded-lg border border-[color:var(--theme-border-accent,var(--color-amber-400))]/25 bg-amber-400/10 px-3 py-2 text-xs text-[color:var(--theme-fg-accent,var(--color-amber-100))]">
                  {t(analysis.blockedReason === "cue" ? "albumTagEditor.cueBlocked"
                    : analysis.blockedReason === "format" ? "albumTagEditor.formatBlocked"
                    : "albumTagEditor.inspectionBlocked")}
                </div>
              )}

              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                {localAlbumTagFields.map((field, index) => {
                  const fieldDraft = draft.fields[field];
                  const state = fieldDraft.summary.state;
                  const disabled = Boolean(applying || consumed.current || analysis?.blocked || !snapshotCurrent || reloading);
                  return (
                    <div key={field} className={`space-y-1.5 ${review?.field === field ? "rounded-lg bg-sky-400/10 p-2 ring-1 ring-[color:var(--theme-border-info,var(--color-sky-400))]/40" : ""}`}>
                      <label htmlFor={`local-album-tag-${field}`} className="text-xs text-[color:var(--theme-fg-secondary,#BBBBBB)]">
                        {t(`tagEditor.fields.${field}`)}
                      </label>
                      <input
                        id={`local-album-tag-${field}`}
                        autoFocus={review ? review.field === field : index === 0}
                        aria-describedby={review?.field === field ? "local-album-tag-doctor-context" : undefined}
                        type="text"
                        value={fieldDraft.value}
                        placeholder={state === "mixed"
                          ? t("albumTagEditor.states.mixed")
                          : state === "absent"
                            ? t("albumTagEditor.states.absent")
                            : ""}
                        onChange={(event) => changeField(field, event.target.value)}
                        disabled={disabled}
                        className="w-full rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-base,#111111)] px-3 py-2 text-sm text-[color:var(--theme-fg-primary,#ffffff)] outline-none placeholder:text-[color:var(--theme-fg-muted,#666666)] focus:border-[color:var(--theme-border-accent,#E5A00D)] disabled:opacity-60"
                      />
                      <div className="flex min-h-5 items-center justify-between gap-2 text-[11px]">
                        <span className={state === "indeterminate" || state === "not_applicable" ? "text-[color:var(--theme-fg-accent,var(--color-amber-300))]" : "text-[color:var(--theme-fg-muted,#666666)]"}>
                          {fieldDraft.clearAll
                            ? t("albumTagEditor.removeForAll")
                            : t(`albumTagEditor.states.${state}`)}
                        </span>
                        {state === "mixed" && (
                          <button
                            type="button"
                            onClick={() => fieldDraft.clearAll ? undoField(field) : clearAll(field)}
                            disabled={disabled}
                            className="flex items-center gap-1 text-[color:var(--theme-fg-secondary,#999999)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] disabled:opacity-50"
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

              <OnlineAlbumMetadataPanel albumId={albumId} catalogVersion={catalogVersion} inspection={draft.inspection}
                available={snapshotCurrent && !mustReload && !loading && !reloading && !previewing && !applying && !consumed.current}
                draft={draft} editorRevision={editorRevision} transferAvailable={!analysis?.blocked && metadataAvailable()}
                isAvailable={metadataAvailable} onTransfer={transferMetadata} />

              {result && (
                <section aria-label={t("albumTagEditor.resultTitle")} className="mt-5 rounded-xl border border-[color:var(--theme-border-soft,#333333)] p-4 text-xs text-[color:var(--theme-fg-secondary,#CCCCCC)]">
                  <h3 className="text-sm font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("albumTagEditor.resultTitle")}</h3>
                  <p className="mt-2">{t("albumTagEditor.resultSummary", {
                    committed: result.committed_count, unchanged: result.unchanged_count,
                    failed: result.failed_count, notAttempted: result.not_attempted_count,
                  })}</p>
                  <p className="mt-2">{t("albumTagEditor.partialCommitStatus", { value: t(result.partial_commit ? "albumTagEditor.yes" : "albumTagEditor.no") })}</p>
                  {result.partial_commit && <p className="mt-2 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">{t("albumTagEditor.partialCommit")}</p>}
                  {!result.applicable && <p className="mt-2 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">{t(result.blocked_reason === "cue_virtual"
                    ? "albumTagEditor.cueBlocked" : "albumTagEditor.inspectionBlocked")}</p>}
                  <p className="mt-2">{t("albumTagEditor.independentWrites")}</p>
                  <ul className="mt-3 space-y-3">
                    {result.tracks.map((track) => (
                      <li key={track.uri} className="rounded-lg border border-[color:var(--theme-border-soft,#333333)] p-3">
                        <p className="break-all font-mono">{track.uri}</p>
                        <p className="mt-1 font-bold">{t(`albumTagEditor.outcomes.${track.state}`)}</p>
                        {track.error && <p className="mt-1 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">
                          {track.state === "committed" && <span>{t("albumTagEditor.committedWarning")} </span>}
                          {t(`albumTagEditor.writeErrors.${track.error.code}`)}
                        </p>}
                        <p className="mt-1">{t(`albumTagEditor.refresh.${track.mpd_refresh.state}`, { jobId: track.mpd_refresh.job_id })}</p>
                        {track.mpd_refresh.error_code && <p className="mt-1 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">
                          {t(`albumTagEditor.refreshErrors.${track.mpd_refresh.error_code}`)}
                        </p>}
                      </li>
                    ))}
                  </ul>
                  <p className="mt-3 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">{t("albumTagEditor.newInspectionRequired")}</p>
                </section>
              )}
              {previewView && (
                <div className={`mt-5 rounded-xl border px-4 py-3 ${
                  previewView.applicable
                    ? "border-[color:var(--theme-border-success,var(--color-emerald-400))]/25 bg-emerald-400/10"
                    : "border-[color:var(--theme-border-accent,var(--color-amber-400))]/25 bg-amber-400/10"
                }`}>
                  <p className={`text-sm font-bold ${previewView.applicable ? "text-[color:var(--theme-fg-success,var(--color-emerald-200))]" : "text-[color:var(--theme-fg-accent,var(--color-amber-100))]"}`}>
                    {t(previewView.applicable ? "albumTagEditor.previewReady" : "albumTagEditor.previewBlocked")}
                  </p>
                  <p className="mt-1 text-xs text-[color:var(--theme-fg-secondary,#BBBBBB)]">
                    {t("albumTagEditor.previewSummary", {
                      changeCount: previewView.changeCount,
                      unchangedCount: previewView.unchangedCount,
                    })}
                  </p>
                  <ul className="mt-3 space-y-3 text-xs">
                    {previewView.files.map((file) => (
                      <li key={file.uri} className="rounded-lg border border-[color:var(--theme-border-soft,#333333)] p-3">
                        <p className="break-all font-mono">{file.uri}</p>
                        {file.blocked ? (
                          <p className="mt-1 text-[color:var(--theme-fg-accent,var(--color-amber-200))]">{t("albumTagEditor.blockedFile")}</p>
                        ) : file.changes.length === 0 ? (
                          <p className="mt-1 text-[color:var(--theme-fg-muted,#888888)]">{t("albumTagEditor.unchangedFile")}</p>
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
                    <p className="mt-2 text-[11px] text-[color:var(--theme-fg-muted,#888888)]">{t("albumTagEditor.previewOnly")}</p>
                  )}
                </div>
              )}
            </div>

            <footer className="flex items-center justify-between gap-3 border-t border-[color:var(--theme-border-soft,#292929)] px-5 py-4">
              <div>
                <button
                  type="button"
                  onClick={reload}
                  disabled={applying || !available || previewing || reloading}
                  className="flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-accent,var(--color-amber-400))]/30 px-3 py-2 text-xs font-bold text-[color:var(--theme-fg-accent,var(--color-amber-200))] hover:bg-amber-400/10 disabled:opacity-50"
                >
                  <RotateCcw size={14} className={reloading ? "animate-spin" : ""} />
                  {t("albumTagEditor.reload")}
                </button>
              </div>
              <div className="flex items-center gap-2">
                {!applying && (consumed.current || result || applyIssue) ? (
                  <button
                    type="button"
                    onClick={requestClose}
                    disabled={applying || previewing}
                    className="rounded-lg border border-[color:var(--theme-border-soft,#333333)] px-4 py-2 text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#252525)] disabled:opacity-50"
                  >{t("albumTagEditor.closeButton")}</button>
                ) : (
                  <>
                    <button
                      type="button"
                      onClick={requestClose}
                      disabled={applying || previewing}
                      className="rounded-lg border border-[color:var(--theme-border-soft,#333333)] px-4 py-2 text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#252525)] disabled:opacity-50"
                    >
                      {t("albumTagEditor.cancel")}
                    </button>
                    {applying || (previewView?.canApply && !mustReload) ? (
                      <button
                        key="apply"
                        type="button"
                        onClick={() => { void requestApply(); }}
                        disabled={applying || consumed.current || !previewView?.canApply || mustReload}
                        className="flex items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black disabled:opacity-40"
                      >
                        {applying && <LoaderCircle size={16} className="animate-spin" />}
                        {t(applying ? "albumTagEditor.applyingButton" : "albumTagEditor.apply")}
                      </button>
                    ) : (
                      <button
                        key="preview"
                        type="submit"
                        disabled={applying || consumed.current || !analysis?.dirty || analysis.blocked || mustReload || previewing || reloading}
                        className="flex items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black hover:bg-[#F5B01D] disabled:cursor-not-allowed disabled:opacity-40"
                      >
                        {previewing ? <LoaderCircle size={16} className="animate-spin" /> : <Eye size={16} />}
                        {t(previewing ? "albumTagEditor.previewing" : "albumTagEditor.preview")}
                      </button>
                    )}
                  </>
                )}
              </div>
            </footer>
          </form>
        ) : (
          <div className="min-h-32 p-5">
            <button type="button" disabled={applying || !available || loading || reloading} onClick={reload}
              className="text-sm text-[color:var(--theme-fg-accent,var(--color-amber-200))] disabled:opacity-40">{t("albumTagEditor.reload")}</button>
          </div>
        )}
      </div>
    </div>
  );
};
